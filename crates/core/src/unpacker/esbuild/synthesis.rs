//! Import/export statement synthesis and owned-declaration filtering shared
//! by scope and factory emission.

use swc_core::atoms::Atom;
use swc_core::common::sync::Lrc;
use swc_core::common::{SourceMap, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, BindingIdent, Bool, CallExpr, Callee, Decl, ExportDecl, Expr,
    ExprOrSpread, ExprStmt, FnDecl, Function, FunctionBody, Ident, IdentName, KeyValueProp, Lit,
    MemberExpr, MemberProp, ModuleDecl, ModuleItem, ObjectLit, Pat, Prop, PropName, PropOrSpread,
    Stmt, Str, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::VisitWith;

use crate::collections::{HashMap, HashSet};
use crate::module_path::relative_import_specifier;
use crate::unpacker::emit_esm::{
    make_named_export_stmt, try_promote_fn_class_export, FilenameDedupStyle,
};
use crate::unpacker::{emit_esm, BindingId, MappedCode, SourcePositions};

use super::bindings::{filter_item_excluding_bindings, ExternalImport, TopLevelRefCollector};

/// Compute a relative import specifier from `importer` to `target`.
/// Both are flat output filenames (e.g. `src/consumer.js`, `ns_a.js`).
/// Returns a string suitable for an ES import source (e.g. `./ns_a.js`,
/// `../ns_a.js`).
pub(super) fn relative_import_path(importer: &str, target: &str) -> String {
    relative_import_specifier(importer, target)
}

pub(super) fn reserve_import_atom(imported: &Atom, reserved: &mut HashSet<Atom>) -> Atom {
    if reserved.insert(imported.clone()) {
        return imported.clone();
    }

    for suffix in 2.. {
        let candidate: Atom = format!("{imported}${suffix}").into();
        if reserved.insert(candidate.clone()) {
            return candidate;
        }
    }

    unreachable!("open-ended suffix search must find an unused import atom")
}

pub(super) fn make_external_import_stmt(import: &ExternalImport) -> ModuleItem {
    let mut decl = import.decl.clone();
    decl.specifiers = vec![import.specifier.clone()];
    ModuleItem::ModuleDecl(ModuleDecl::Import(decl))
}

pub(super) fn make_namespace_define_property_items(
    namespace: &Atom,
    entries: &[(Atom, BindingId)],
) -> Vec<ModuleItem> {
    let mut items = Vec::new();
    let mut seen = HashSet::default();
    for (export_name, (binding_name, _)) in entries {
        if !seen.insert(export_name.clone()) {
            continue;
        }
        items.push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(Expr::Call(CallExpr {
                span: DUMMY_SP,
                ctxt: SyntaxContext::empty(),
                callee: Callee::Expr(Box::new(Expr::Member(MemberExpr {
                    span: DUMMY_SP,
                    obj: Box::new(Expr::Ident(Ident::new(
                        "Object".into(),
                        DUMMY_SP,
                        SyntaxContext::empty(),
                    ))),
                    prop: MemberProp::Ident(IdentName::new("defineProperty".into(), DUMMY_SP)),
                }))),
                args: vec![
                    ExprOrSpread {
                        spread: None,
                        expr: Box::new(Expr::Ident(Ident::new(
                            namespace.clone(),
                            DUMMY_SP,
                            SyntaxContext::empty(),
                        ))),
                    },
                    ExprOrSpread {
                        spread: None,
                        expr: Box::new(Expr::Lit(Lit::Str(Str {
                            span: DUMMY_SP,
                            value: export_name.clone().into(),
                            raw: None,
                        }))),
                    },
                    ExprOrSpread {
                        spread: None,
                        expr: Box::new(Expr::Object(ObjectLit {
                            span: DUMMY_SP,
                            props: vec![
                                PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                                    key: PropName::Ident(IdentName::new(
                                        "enumerable".into(),
                                        DUMMY_SP,
                                    )),
                                    value: Box::new(Expr::Lit(Lit::Bool(Bool {
                                        span: DUMMY_SP,
                                        value: true,
                                    }))),
                                }))),
                                PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                                    key: PropName::Ident(IdentName::new("get".into(), DUMMY_SP)),
                                    value: Box::new(Expr::Arrow(ArrowExpr {
                                        span: DUMMY_SP,
                                        ctxt: SyntaxContext::empty(),
                                        params: Vec::new(),
                                        body: Box::new(ArrowFunctionBody::Expr(Box::new(
                                            Expr::Ident(Ident::new(
                                                binding_name.clone(),
                                                DUMMY_SP,
                                                SyntaxContext::empty(),
                                            )),
                                        ))),
                                        is_async: false,
                                        is_generator: false,
                                        type_params: None,
                                        return_type: None,
                                    })),
                                }))),
                            ],
                        })),
                    },
                ],
                type_args: None,
            })),
        })));
    }
    items
}

/// `export var {namespace} = {};` — the seed object a namespace's
/// `Object.defineProperty` getters attach to.
pub(super) fn make_namespace_object_decl(namespace: &Atom) -> ModuleItem {
    ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
        span: Default::default(),
        decl: Decl::Var(Box::new(VarDecl {
            span: DUMMY_SP,
            ctxt: SyntaxContext::empty(),
            kind: VarDeclKind::Var,
            declare: false,
            decls: vec![VarDeclarator {
                span: DUMMY_SP,
                name: Pat::Ident(BindingIdent {
                    id: Ident::new(namespace.clone(), DUMMY_SP, SyntaxContext::empty()),
                    type_ann: None,
                }),
                init: Some(Box::new(Expr::Object(ObjectLit {
                    span: DUMMY_SP,
                    props: Vec::new(),
                }))),
                definite: false,
            }],
        })),
    }))
}

pub(super) fn factory_owned_decl_items(
    filename: &str,
    factory_owned_bindings: &HashMap<String, HashSet<BindingId>>,
    top_level_decl_indices: &HashMap<BindingId, usize>,
    source_items: &[ModuleItem],
) -> Vec<ModuleItem> {
    factory_owned_decl_items_from(
        filename,
        factory_owned_bindings,
        top_level_decl_indices,
        source_items,
    )
}

pub(super) fn scope_owned_support_decl_items(
    owned: &HashSet<BindingId>,
    decl_index_by_binding: &HashMap<BindingId, usize>,
    source_items: &HashMap<usize, ModuleItem>,
) -> Vec<ModuleItem> {
    if owned.is_empty() {
        return vec![];
    }
    let owned_atoms: HashSet<Atom> = owned.iter().map(|(atom, _)| atom.clone()).collect();
    let mut item_indices: Vec<(usize, ModuleItem)> = owned
        .iter()
        .filter_map(|binding| {
            decl_index_by_binding
                .get(binding)
                .and_then(|index| source_items.get(index).map(|item| (*index, item.clone())))
        })
        .collect();
    item_indices.sort_by_key(|(index, _)| *index);
    item_indices.dedup_by_key(|(index, _)| *index);
    item_indices
        .into_iter()
        .filter_map(|(_, item)| filter_item_to_owned_bindings(&item, &owned_atoms))
        .collect()
}

pub(super) fn retain_owned_support_source_items(
    source_slots: &mut [Option<ModuleItem>],
    owned_support_by_index: &HashMap<usize, HashSet<BindingId>>,
) -> HashMap<usize, ModuleItem> {
    let mut originals =
        HashMap::with_capacity_and_hasher(owned_support_by_index.len(), Default::default());
    for (index, owned) in owned_support_by_index {
        let owned_atoms: HashSet<Atom> = owned.iter().map(|(atom, _)| atom.clone()).collect();
        if let Some(item) = source_slots[*index].take() {
            originals.insert(*index, item.clone());
            source_slots[*index] = filter_item_excluding_bindings(&item, owned, &owned_atoms);
        }
    }
    originals
}

fn factory_owned_decl_items_from(
    filename: &str,
    factory_owned_bindings: &HashMap<String, HashSet<BindingId>>,
    top_level_decl_indices: &HashMap<BindingId, usize>,
    items: &[ModuleItem],
) -> Vec<ModuleItem> {
    let Some(owned) = factory_owned_bindings.get(filename) else {
        return vec![];
    };
    let owned_atoms: HashSet<Atom> = owned.iter().map(|(atom, _)| atom.clone()).collect();
    let mut item_indices: Vec<(usize, ModuleItem)> = owned
        .iter()
        .filter_map(|binding| {
            top_level_decl_indices
                .get(binding)
                .map(|index| (*index, items[*index].clone()))
        })
        .collect();
    item_indices.sort_by_key(|(index, _)| *index);
    item_indices.dedup_by_key(|(index, _)| *index);
    item_indices
        .into_iter()
        .filter_map(|(_, item)| filter_item_to_owned_bindings(&item, &owned_atoms))
        .collect()
}

pub(super) fn factory_owned_export_items(
    filename: &str,
    factory_owned_bindings: &HashMap<String, HashSet<BindingId>>,
) -> Vec<ModuleItem> {
    let Some(owned) = factory_owned_bindings.get(filename) else {
        return vec![];
    };
    let mut names: Vec<Atom> = owned.iter().map(|(atom, _)| atom.clone()).collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        vec![]
    } else {
        vec![make_named_export_stmt(&names)]
    }
}

pub(super) fn filter_item_to_owned_bindings(
    item: &ModuleItem,
    owned_atoms: &HashSet<Atom>,
) -> Option<ModuleItem> {
    let decl = match item {
        ModuleItem::Stmt(Stmt::Decl(decl)) => decl,
        // Move only the declaration. Factory modules synthesize their own
        // exports, and scope modules promote the filtered declaration later.
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => &export_decl.decl,
        _ => return None,
    };
    filter_decl_to_owned_bindings(decl, owned_atoms).map(|decl| ModuleItem::Stmt(Stmt::Decl(decl)))
}

fn filter_decl_to_owned_bindings(decl: &Decl, owned_atoms: &HashSet<Atom>) -> Option<Decl> {
    match decl {
        Decl::Fn(fn_decl) if owned_atoms.contains(&fn_decl.ident.sym) => {
            Some(Decl::Fn(fn_decl.clone()))
        }
        Decl::Class(class_decl) if owned_atoms.contains(&class_decl.ident.sym) => {
            Some(Decl::Class(class_decl.clone()))
        }
        Decl::Var(var_decl) => {
            let stmt_bindings: HashSet<BindingId> = var_decl
                .decls
                .iter()
                .flat_map(|decl| pat_declared_binding_ids(&decl.name))
                .collect();
            let mut keep_atoms: HashSet<Atom> = HashSet::default();
            for decl in &var_decl.decls {
                let decl_atoms: Vec<Atom> = pat_declared_binding_ids(&decl.name)
                    .into_iter()
                    .map(|(atom, _)| atom)
                    .collect();
                if !decl_atoms.iter().any(|atom| owned_atoms.contains(atom)) {
                    continue;
                }
                keep_atoms.extend(decl_atoms);
                let mut collector = TopLevelRefCollector {
                    top_level_bindings: &stmt_bindings,
                    references: HashSet::default(),
                };
                decl.visit_with(&mut collector);
                keep_atoms.extend(collector.references.into_iter().map(|(atom, _)| atom));
            }

            if var_decl
                .decls
                .iter()
                .any(|decl| pat_declares_owned(&decl.name, &keep_atoms))
            {
                // A minified declaration can contain many large sibling
                // initializers. Clone only the selected ownership unit, not
                // every sibling followed by dropping the rejected subtrees.
                Some(Decl::Var(Box::new(VarDecl {
                    span: var_decl.span,
                    ctxt: var_decl.ctxt,
                    kind: var_decl.kind,
                    declare: var_decl.declare,
                    decls: var_decl
                        .decls
                        .iter()
                        .filter(|decl| pat_declares_owned(&decl.name, &keep_atoms))
                        .cloned()
                        .collect(),
                })))
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(super) fn pat_declared_binding_ids(pat: &Pat) -> Vec<BindingId> {
    find_pat_ids(pat)
}

fn pat_declares_owned(pat: &Pat, owned_atoms: &HashSet<Atom>) -> bool {
    match pat {
        Pat::Ident(bi) => owned_atoms.contains(&bi.id.sym),
        Pat::Array(array) => array
            .elems
            .iter()
            .flatten()
            .any(|elem| pat_declares_owned(elem, owned_atoms)),
        Pat::Object(object) => object.props.iter().any(|prop| match prop {
            swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                pat_declares_owned(&kv.value, owned_atoms)
            }
            swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                owned_atoms.contains(&assign.key.sym)
            }
            swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                pat_declares_owned(&rest.arg, owned_atoms)
            }
        }),
        Pat::Assign(assign) => pat_declares_owned(&assign.left, owned_atoms),
        Pat::Rest(rest) => pat_declares_owned(&rest.arg, owned_atoms),
        _ => false,
    }
}

pub(super) enum ScopeExportPromotion {
    Promoted(ModuleItem, Vec<Atom>),
    Unchanged(ModuleItem),
}

pub(super) fn try_promote_scope_export(
    item: ModuleItem,
    exported: &HashSet<Atom>,
) -> ScopeExportPromotion {
    if let Some((new_item, names)) = try_promote_fn_class_export(&item, exported) {
        return ScopeExportPromotion::Promoted(new_item, names);
    }
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
            if var_decl.decls.len() == 1 {
                let decl = &var_decl.decls[0];
                if let Pat::Ident(bi) = &decl.name {
                    if exported.contains(&bi.id.sym)
                        && decl.init.as_deref().is_some_and(is_noop_arrow_expr)
                    {
                        let names = vec![bi.id.sym.clone()];
                        return ScopeExportPromotion::Promoted(
                            make_noop_export_function(&bi.id.sym),
                            names,
                        );
                    }
                }
            }
            let all_exported = var_decl
                .decls
                .iter()
                .all(|d| matches!(&d.name, Pat::Ident(bi) if exported.contains(&bi.id.sym)));
            if !all_exported {
                return ScopeExportPromotion::Unchanged(ModuleItem::Stmt(Stmt::Decl(Decl::Var(
                    var_decl,
                ))));
            }
            let names: Vec<Atom> = var_decl
                .decls
                .iter()
                .filter_map(|d| {
                    if let Pat::Ident(bi) = &d.name {
                        Some(bi.id.sym.clone())
                    } else {
                        Option::None
                    }
                })
                .collect();
            if names.is_empty() {
                return ScopeExportPromotion::Unchanged(ModuleItem::Stmt(Stmt::Decl(Decl::Var(
                    var_decl,
                ))));
            }
            ScopeExportPromotion::Promoted(
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                    span: Default::default(),
                    decl: Decl::Var(var_decl),
                })),
                names,
            )
        }
        item => ScopeExportPromotion::Unchanged(item),
    }
}

fn is_noop_arrow_expr(expr: &Expr) -> bool {
    let Expr::Arrow(ArrowExpr { params, body, .. }) = expr else {
        return false;
    };
    params.is_empty()
        && matches!(
            &**body,
            ArrowFunctionBody::FunctionBody(block) if block.stmts.is_empty()
        )
}

fn make_noop_export_function(name: &Atom) -> ModuleItem {
    ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
        span: Default::default(),
        decl: Decl::Fn(FnDecl {
            ident: Ident::new(name.clone(), Default::default(), Default::default()),
            declare: false,
            function: Box::new(Function {
                params: vec![],
                decorators: vec![],
                span: Default::default(),
                ctxt: Default::default(),
                this_param: None,
                body: Some(FunctionBody {
                    span: Default::default(),
                    stmts: vec![],
                }),
                is_generator: false,
                is_async: false,
                type_params: None,
                return_type: None,
            }),
        }),
    }))
}

/// Case-insensitive filename dedup matching the CLI's `deduplicate_path`
/// logic (see `emit_esm::FilenameDedupStyle` for the pointer to that copy).
pub(super) fn dedup_filename(filename: &str, seen: &mut HashSet<String>) -> String {
    emit_esm::dedup_filename(filename, seen, FilenameDedupStyle::Flat)
}

pub(super) fn emit_items(
    items: Vec<ModuleItem>,
    filename: String,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> MappedCode {
    let span = tracing::info_span!("esbuild: emit_items", count = items.len());
    let _enter = span.enter();
    emit_esm::emit_items(items, filename, cm, positions)
}
