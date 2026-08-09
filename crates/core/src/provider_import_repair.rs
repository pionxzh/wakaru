//! Repair CommonJS property requires using proven provider facts.
//!
//! `UnEsm` must classify `require("./provider").name` before cross-module
//! facts exist, so it initially emits a named import. When the provider is
//! later proven to originate from `module.exports = {...}` with that statically
//! declared property, the CommonJS operation was a property capture, not a
//! live named binding:
//!
//! ```text
//! import { name } from "./provider.js";
//! // becomes
//! import _name from "./provider.js";
//! var name = _name.name;
//! ```
//!
//! The pass is deliberately narrow. It touches only import declarations
//! synthesized by `UnEsm` (dummy span), requires a statically proven raw
//! CommonJS default-object property, and leaves export-star providers and
//! unknown default values unchanged.

use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{BytePos, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    BindingIdent, Decl, Expr, Ident, IdentName, ImportDefaultSpecifier, ImportNamedSpecifier,
    ImportSpecifier, MemberExpr, MemberProp, Module, ModuleDecl, ModuleExportName, ModuleItem, Pat,
    Stmt, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;
use crate::analysis::{binding_id, BindingId};
use crate::facts::{ExportKind, ModuleFactsMap};

enum CaptureSite {
    OriginalPosition(BytePos),
    ExistingMutableAlias {
        body_index: usize,
        declarator_index: usize,
    },
}

pub(crate) fn run_provider_import_repair(
    module: &mut Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
) {
    let Some(current_filename) = current_filename else {
        return;
    };

    let mut names = IdentifierNameCollector::default();
    module.visit_with(&mut names);
    let mut used_names = names.names;
    let binding_uses = BindingUseIndex::collect(module);
    let mutable_aliases = collect_existing_mutable_aliases(module, &binding_uses);
    let mut captures = Vec::new();
    let mut alias_rewrites = Vec::new();

    for item in &mut module.body {
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
        if provider.has_export_all
            || provider.default_object_properties.is_empty()
            || !provider
                .exports
                .iter()
                .any(|export| export.kind == ExportKind::Default)
        {
            continue;
        }

        let named_exports = provider
            .exports
            .iter()
            .filter(|export| export.kind == ExportKind::Named)
            .map(|export| export.exported.as_ref())
            .collect::<HashSet<_>>();
        let default_properties = provider
            .default_object_properties
            .iter()
            .map(Atom::as_ref)
            .collect::<HashSet<_>>();

        let mut retained = Vec::with_capacity(import.specifiers.len() + 1);
        let mut repaired = Vec::new();
        for specifier in std::mem::take(&mut import.specifiers) {
            let ImportSpecifier::Named(named) = &specifier else {
                retained.push(specifier);
                continue;
            };
            let Some(property) = imported_identifier_name(named) else {
                retained.push(specifier);
                continue;
            };
            let capture_site = if !named.local.span.is_dummy() {
                Some(CaptureSite::OriginalPosition(named.local.span.lo))
            } else {
                mutable_aliases.get(&binding_id(&named.local)).map(
                    |&(body_index, declarator_index)| CaptureSite::ExistingMutableAlias {
                        body_index,
                        declarator_index,
                    },
                )
            };
            if named.is_type_only
                || capture_site.is_none()
                || named_exports.contains(property.as_ref())
                || !default_properties.contains(property.as_ref())
            {
                retained.push(specifier);
                continue;
            }
            repaired.push((named.local.clone(), property, capture_site.unwrap()));
        }

        if repaired.is_empty() {
            import.specifiers = retained;
            continue;
        }

        let default_local = retained.iter().find_map(|specifier| match specifier {
            ImportSpecifier::Default(default) => Some(default.local.clone()),
            _ => None,
        });
        let default_local = default_local.unwrap_or_else(|| {
            let local = Ident::new(
                fresh_capture_name(&repaired[0].0.sym, &mut used_names),
                DUMMY_SP,
                SyntaxContext::empty(),
            );
            retained.insert(
                0,
                ImportSpecifier::Default(ImportDefaultSpecifier {
                    span: DUMMY_SP,
                    local: local.clone(),
                }),
            );
            local
        });
        import.specifiers = retained;

        for (local, property, capture_site) in repaired {
            match capture_site {
                CaptureSite::OriginalPosition(origin) => captures.push((
                    origin,
                    property_capture_item(local, default_local.clone(), property),
                )),
                CaptureSite::ExistingMutableAlias {
                    body_index,
                    declarator_index,
                } => alias_rewrites.push((
                    body_index,
                    declarator_index,
                    default_local.clone(),
                    property,
                )),
            }
        }
    }

    if captures.is_empty() && alias_rewrites.is_empty() {
        return;
    }
    for (body_index, declarator_index, object, property) in alias_rewrites {
        let Some(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var)))) = module.body.get_mut(body_index)
        else {
            unreachable!("proven mutable capture declaration must remain in place")
        };
        var.decls[declarator_index].init = Some(property_member_expr(object, property));
    }

    let first_non_import = module
        .body
        .iter()
        .take_while(|item| matches!(item, ModuleItem::ModuleDecl(ModuleDecl::Import(_))))
        .count();
    captures.sort_by_key(|(origin, _)| *origin);
    for (origin, capture) in captures {
        let insert_at = module
            .body
            .iter()
            .enumerate()
            .skip(first_non_import)
            .find_map(|(index, item)| {
                let span = item.span();
                (!span.is_dummy() && span.lo > origin).then_some(index)
            })
            .unwrap_or(module.body.len());
        module.body.insert(insert_at, capture);
    }
}

fn collect_existing_mutable_aliases(
    module: &Module,
    uses: &BindingUseIndex,
) -> HashMap<BindingId, (usize, usize)> {
    // `UnEsm` already protects reassigned require locals by importing into a
    // fresh dummy-span binding and initializing the original mutable local at
    // its source position. Reuse that declaration instead of adding a second
    // capture or moving it across intervening effects.
    let mut aliases = HashMap::new();
    for (body_index, item) in module.body.iter().enumerate() {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for (declarator_index, declarator) in var.decls.iter().enumerate() {
            let Some(Expr::Ident(import_local)) = declarator.init.as_deref() else {
                continue;
            };
            let import_binding = binding_id(import_local);
            if uses.use_count(&import_binding) == 1 {
                aliases.insert(import_binding, (body_index, declarator_index));
            }
        }
    }
    aliases
}

fn imported_identifier_name(named: &ImportNamedSpecifier) -> Option<Atom> {
    match &named.imported {
        Some(ModuleExportName::Ident(imported)) => Some(imported.sym.clone()),
        Some(ModuleExportName::Str(_)) => None,
        None => Some(named.local.sym.clone()),
    }
}

fn fresh_capture_name(preferred: &Atom, used_names: &mut HashSet<Atom>) -> Atom {
    let base: Atom = format!("_{preferred}").into();
    if used_names.insert(base.clone()) {
        return base;
    }
    for suffix in 1usize.. {
        let candidate: Atom = format!("{base}_{suffix}").into();
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

fn property_capture_item(local: Ident, object: Ident, property: Atom) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent::from(local)),
            init: Some(property_member_expr(object, property)),
            definite: false,
        }],
    }))))
}

fn property_member_expr(object: Ident, property: Atom) -> Box<Expr> {
    Box::new(Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(Expr::Ident(object)),
        prop: MemberProp::Ident(IdentName::new(property, DUMMY_SP)),
    }))
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
