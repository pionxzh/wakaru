//! Factory output emission: standalone groups, merged scope-module
//! additions, compatibility redirects, and entry.js. Reads settled ownership.

use swc_core::atoms::Atom;
use swc_core::common::sync::Lrc;
use swc_core::common::{SourceMap, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BindingIdent, Bool, Decl, Expr, ExprStmt, FnDecl, Function,
    FunctionBody, Ident, IdentName, IfStmt, KeyValueProp, Lit, MemberExpr, MemberProp, Module,
    ModuleDecl, ModuleItem, ObjectLit, Pat, Prop, PropName, PropOrSpread, ReturnStmt,
    SimpleAssignTarget, Stmt, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::collections::{HashMap, HashSet};
use crate::rules::rename_utils::{rename_bindings, BindingRename};
use crate::unpacker::emit_esm::{make_named_import_stmt, make_named_import_stmt_with_aliases};
use crate::unpacker::{
    emit_esm, module_item_declared_binding_ids, span_byte_range, spans_byte_ranges, BindingId,
    MappedCode, SourcePositions, UnpackedModule,
};

use super::bindings::{
    atom_to_filename_binding_map, filter_item_excluding_bindings, module_item_import_binding_ids,
    AtomRefCollector, ExternalImport,
};
use super::factories::CjsFactoryParams;
use super::ownership::{
    relocated_entry_bindings, FactoryOwnership, MergedModulePlan, PendingFactory, TopLevelIndex,
    TopLevelWriterItem,
};
use super::synthesis::{
    emit_items, export_items, factory_owned_decl_items, factory_owned_export_names,
    filter_item_to_owned_bindings, make_external_import_stmt, relative_import_path,
    reserve_import_atom,
};

/// Re-synthesizes demoted factories into the entry at their source positions
/// so entry call sites stay after the definition.
pub(super) fn restore_demoted_factories(
    remaining_entry: &mut Vec<ModuleItem>,
    demoted_factories: Vec<PendingFactory>,
) {
    // The synthesized cache/guard joins the entry's top-level scope.
    // Reserve every name the entry already declares or imports, plus
    // the restored factory names, so the helper cannot shadow a `var`
    // (silently skipping the body) or duplicate a lexical binding.
    let mut reserved_entry_atoms: HashSet<Atom> = remaining_entry
        .iter()
        .flat_map(|item| {
            module_item_declared_binding_ids(item)
                .into_iter()
                .chain(module_item_import_binding_ids(item))
        })
        .map(|(atom, _)| atom)
        .collect();
    reserved_entry_atoms.extend(
        demoted_factories
            .iter()
            .map(|factory| factory.var_name.clone()),
    );
    let mut restored_items: Vec<(u32, ModuleItem)> = Vec::new();
    for factory in demoted_factories {
        // Body locals, parameters, and free references would
        // shadow the helper inside the restored callable.
        reserved_entry_atoms.extend(ident_atoms_in_stmts(&factory.body_stmts));
        restored_items.extend(match &factory.cjs_params {
            Some(cjs_params) => {
                let cache = reserve_import_atom(
                    &format!("__wakaru_{}_cache", factory.var_name).into(),
                    &mut reserved_entry_atoms,
                );
                synthesize_entry_cjs_items(
                    &factory.var_name,
                    &cache,
                    cjs_params,
                    factory.body_stmts,
                    factory.span,
                )
            }
            None => {
                let guard = reserve_import_atom(
                    &format!("__wakaru_{}_initialized", factory.var_name).into(),
                    &mut reserved_entry_atoms,
                );
                synthesize_entry_init_items(
                    &factory.var_name,
                    &guard,
                    factory.body_stmts,
                    factory.span,
                )
            }
        });
    }
    for (position, item) in restored_items {
        let index = remaining_entry
            .iter()
            .position(|existing| existing.span().lo.0 > position)
            .unwrap_or(remaining_entry.len());
        remaining_entry.insert(index, item);
    }
}

/// Groups standalone factories by output file, in first-seen order.
pub(super) fn group_standalone_factories(
    standalone_factories: Vec<PendingFactory>,
) -> Vec<(String, Vec<PendingFactory>)> {
    let mut group_order = Vec::new();
    let mut groups: HashMap<String, Vec<PendingFactory>> = HashMap::default();
    for factory in standalone_factories {
        if !groups.contains_key(&factory.filename) {
            group_order.push(factory.filename.clone());
        }
        groups
            .entry(factory.filename.clone())
            .or_default()
            .push(factory);
    }
    group_order
        .into_iter()
        .map(|filename| {
            let factories = groups
                .remove(&filename)
                .expect("standalone factory group should exist");
            (filename, factories)
        })
        .collect()
}

/// Emits one standalone factory group: imports, owned support declarations
/// and relocated writers, exports of owned state, then the init callables.
pub(super) fn emit_standalone_group(
    group_filename: String,
    factories: Vec<PendingFactory>,
    source_items: &[ModuleItem],
    index: &TopLevelIndex,
    ownership: &FactoryOwnership,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> UnpackedModule {
    let binding_to_filename = &ownership.binding_to_filename;
    let factory_owned_bindings = &ownership.factory_owned_bindings;
    let relocated_writer_items: &[TopLevelWriterItem] = ownership
        .relocated_writers
        .get(&group_filename)
        .map_or(&[], Vec::as_slice);
    let group_id = factories[0].var_name.to_string();
    let mut group_source_ranges: Vec<_> = factories
        .iter()
        .filter_map(|factory| span_byte_range(&cm, factory.span))
        .collect();
    let group_write_bindings: HashSet<BindingId> = factories
        .iter()
        .flat_map(|factory| factory.write_bindings.iter().cloned())
        .collect();
    let mut owned_prelude_items: Vec<(u32, ModuleItem)> = factory_owned_decl_items(
        &group_filename,
        factory_owned_bindings,
        &index.decl_indices,
        source_items,
    )
    .into_iter()
    .map(|item| (item.span().lo.0, item))
    .collect();
    owned_prelude_items.extend(relocated_writer_items.iter().map(|writer| {
        let item = source_items[writer.source_index].clone();
        (item.span().lo.0, item)
    }));
    owned_prelude_items.sort_by_key(|(position, _)| *position);
    group_source_ranges.extend(spans_byte_ranges(
        &cm,
        owned_prelude_items.iter().map(|(_, item)| item.span()),
    ));
    let declared_owned_atoms: HashSet<Atom> = owned_prelude_items
        .iter()
        .flat_map(|(_, item)| module_item_declared_binding_ids(item))
        .map(|(atom, _)| atom)
        .collect();
    let owned_export_atoms: HashSet<Atom> = factory_owned_bindings
        .get(&group_filename)
        .into_iter()
        .flat_map(|bindings| bindings.iter())
        .map(|(atom, _)| atom.clone())
        .collect();
    let mut extended_referenced_bindings: HashSet<BindingId> = factories
        .iter()
        .flat_map(|factory| factory.referenced_bindings.iter().cloned())
        .collect();
    extended_referenced_bindings.extend(
        relocated_writer_items
            .iter()
            .flat_map(|writer| writer.referenced_bindings.iter().cloned()),
    );
    for owned_binding in factory_owned_bindings
        .get(&group_filename)
        .into_iter()
        .flatten()
    {
        if let Some(references) = index.decl_references.get(owned_binding) {
            extended_referenced_bindings.extend(references.iter().cloned());
        }
    }

    let mut import_items: Vec<ModuleItem> = Vec::new();
    let mut external_import_bindings: HashSet<BindingId> = HashSet::default();
    let mut import_renames: Vec<BindingRename> = Vec::new();

    if !binding_to_filename.is_empty() {
        // Group factory's referenced bindings by source module filename.
        let mut imports_by_source: HashMap<String, Vec<BindingId>> = HashMap::default();
        let owned = factory_owned_bindings
            .get(&group_filename)
            .cloned()
            .unwrap_or_default();
        for ref_binding in &extended_referenced_bindings {
            // Don't import bindings that this factory writes to.
            if group_write_bindings.contains(ref_binding)
                || owned.contains(ref_binding)
                || declared_owned_atoms.contains(&ref_binding.0)
            {
                continue;
            }
            if let Some(source_filename) = binding_to_filename.get(ref_binding) {
                if source_filename == &group_filename {
                    continue;
                }
                imports_by_source
                    .entry(source_filename.clone())
                    .or_default()
                    .push(ref_binding.clone());
            } else if index.external_imports.contains_key(ref_binding) {
                external_import_bindings.insert(ref_binding.clone());
            }
        }
        let mut reserved_import_atoms = declared_owned_atoms.clone();
        reserved_import_atoms.extend(owned_export_atoms.iter().cloned());
        reserved_import_atoms.extend(group_write_bindings.iter().map(|(atom, _)| atom.clone()));
        let mut source_filenames: Vec<String> = imports_by_source.keys().cloned().collect();
        source_filenames.sort();
        for source_filename in source_filenames {
            let bindings = imports_by_source.get_mut(&source_filename).unwrap();
            bindings.sort_by(|a, b| a.0.cmp(&b.0));
            bindings.dedup();
            let mut names = Vec::new();
            for binding in bindings {
                let imported = binding.0.clone();
                let local = reserve_import_atom(&imported, &mut reserved_import_atoms);
                if local != imported {
                    import_renames.push(BindingRename {
                        old: binding.clone(),
                        new: local.clone(),
                    });
                }
                names.push((imported, local));
            }
            let rel_path = relative_import_path(&group_filename, &source_filename);
            import_items.push(make_named_import_stmt_with_aliases(&names, &rel_path));
        }
    }
    let mut external_import_bindings: Vec<BindingId> =
        external_import_bindings.into_iter().collect();
    external_import_bindings.sort_by(|a, b| a.0.cmp(&b.0));
    for binding in external_import_bindings {
        if let Some(import) = index.external_imports.get(&binding) {
            import_items.push(make_external_import_stmt(import));
        }
    }

    let mut body_items: Vec<ModuleItem> = import_items
        .into_iter()
        .chain(owned_prelude_items.into_iter().map(|(_, item)| item))
        .chain(export_items(&factory_owned_export_names(
            &group_filename,
            factory_owned_bindings,
        )))
        .collect();
    rename_bindings(&mut body_items, &import_renames);
    let mut reserved_helper_atoms: HashSet<Atom> = body_items
        .iter()
        .flat_map(|item| {
            module_item_declared_binding_ids(item)
                .into_iter()
                .chain(module_item_import_binding_ids(item))
        })
        .map(|(atom, _)| atom)
        .collect();
    reserved_helper_atoms.extend(owned_export_atoms.iter().cloned());
    reserved_helper_atoms.extend(group_write_bindings.iter().map(|(atom, _)| atom.clone()));
    reserved_helper_atoms.extend(factories.iter().map(|factory| factory.var_name.clone()));

    let mut write_names: Vec<Atom> = group_write_bindings
        .iter()
        .filter(|binding| {
            binding_to_filename
                .get(*binding)
                .is_some_and(|filename| filename == &group_filename)
                && !declared_owned_atoms.contains(&binding.0)
        })
        .map(|(atom, _)| atom.clone())
        .collect();
    write_names.sort();
    write_names.dedup();

    let mut code = MappedCode::default();
    // Other modules may import and call this synthetic init function while
    // this module is still evaluating through an ESM cycle. Keep the
    // module-local storage it mutates before the exported callable wrapper,
    // otherwise later VarDeclToLetConst can turn trailing `var` storage
    // into TDZ-sensitive `let` declarations.
    if !body_items.is_empty() {
        code.push_mapped(emit_items(
            body_items,
            group_filename.clone(),
            cm.clone(),
            positions,
        ));
    }
    if !write_names.is_empty() {
        let names = write_names
            .iter()
            .map(|name| name.as_ref())
            .collect::<Vec<_>>()
            .join(", ");
        code.push_str(&format!("export var {names};\n"));
    }
    for mut factory in factories {
        rename_bindings(&mut factory.body_stmts, &import_renames);
        let factory_body_stmts = std::mem::take(&mut factory.body_stmts);
        code.push_mapped(emit_factory_function_code(
            &factory.var_name,
            factory.cjs_params.as_ref(),
            factory_body_stmts,
            &mut reserved_helper_atoms,
            group_filename.clone(),
            cm.clone(),
            positions,
        ));
    }
    UnpackedModule {
        id: group_id,
        is_entry: false,
        code: code.code,
        filename: group_filename,
        source_ranges: group_source_ranges,
        inspection_context_ranges: Vec::new(),
        source_input: String::new(),
        generated_source_map: code.points,
        verbatim_source_offset: None,
        mapped_in_every_mode: false,
    }
}

/// Compatibility files for writer-group members that are not canonical: each
/// re-exports its canonical group file.
pub(super) fn redirect_stub_modules(redirects: &HashMap<String, String>) -> Vec<UnpackedModule> {
    let mut factory_redirects: Vec<(&String, &String)> = redirects.iter().collect();
    factory_redirects.sort();
    factory_redirects
        .into_iter()
        .map(|(filename, canonical)| {
            let specifier = relative_import_path(filename, canonical);
            UnpackedModule {
                id: filename.strip_suffix(".js").unwrap_or(filename).to_string(),
                is_entry: false,
                code: format!("export * from \"{specifier}\";\n"),
                filename: filename.clone(),
                source_ranges: Vec::new(),
                inspection_context_ranges: Vec::new(),
                source_input: String::new(),
                generated_source_map: Vec::new(),
                verbatim_source_offset: None,
                mapped_in_every_mode: false,
            }
        })
        .collect()
}

/// Emits entry.js: the remaining top-level items plus restored demoted
/// factories, without relocated declarations and writers, with imports
/// repaired against the final owners.
pub(super) fn emit_entry(
    mut remaining_entry: Vec<ModuleItem>,
    demoted_factories: Vec<PendingFactory>,
    standalone_factory_write_bindings: &HashSet<BindingId>,
    index: &TopLevelIndex,
    ownership: &FactoryOwnership,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> UnpackedModule {
    restore_demoted_factories(&mut remaining_entry, demoted_factories);
    let relocated_entry_bindings =
        relocated_entry_bindings(standalone_factory_write_bindings, index, ownership);
    if !relocated_entry_bindings.is_empty() {
        let relocated_entry_atoms: HashSet<Atom> = relocated_entry_bindings
            .iter()
            .map(|(atom, _)| atom.clone())
            .collect();
        // Entry imports that still name any original file of an affected
        // group, canonical or redirected, point at the relocated state.
        let factory_import_specifiers: HashSet<String> = ownership
            .affected_group_files()
            .map(|filename| relative_import_path("entry.js", filename))
            .collect();
        remaining_entry = remaining_entry
            .into_iter()
            .filter_map(|item| {
                if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = &item {
                    if import
                        .src
                        .value
                        .as_str()
                        .is_some_and(|source| factory_import_specifiers.contains(source))
                    {
                        return None;
                    }
                }
                filter_item_excluding_bindings(
                    &item,
                    &relocated_entry_bindings,
                    &relocated_entry_atoms,
                )
            })
            .collect();
    }
    let relocated_factory_writer_spans: HashSet<(u32, u32)> = ownership
        .relocated_writers
        .values()
        .flatten()
        .map(|writer| (writer.span.lo.0, writer.span.hi.0))
        .collect();
    if !relocated_factory_writer_spans.is_empty() {
        remaining_entry.retain(|item| {
            !relocated_factory_writer_spans.contains(&(item.span().lo.0, item.span().hi.0))
        });
    }
    let entry_ranges = spans_byte_ranges(&cm, remaining_entry.iter().map(|item| item.span()));
    let remaining_entry = repair_entry_imports(remaining_entry, &ownership.binding_to_filename);
    let entry_module = Module {
        span: Default::default(),
        body: remaining_entry,
        shebang: None,
    };
    let code = emit_esm::emit_module(entry_module, "entry.js".to_string(), cm, positions);
    UnpackedModule {
        id: "entry".to_string(),
        is_entry: true,
        code: code.code,
        filename: "entry.js".to_string(),
        source_ranges: entry_ranges,
        inspection_context_ranges: Vec::new(),
        source_input: String::new(),
        generated_source_map: code.points,
        verbatim_source_offset: None,
        mapped_in_every_mode: false,
    }
}

/// Appends a planned merge to its scope module: synthesized imports, adopted
/// declarations and their export, then the init callables.
pub(super) fn emit_merged_module_plan(
    module: &mut UnpackedModule,
    plan: MergedModulePlan,
    source_items: &[ModuleItem],
    external_imports: &HashMap<BindingId, ExternalImport>,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) {
    let MergedModulePlan {
        external_imports: external_import_bindings,
        named_imports,
        owned_items,
        export_names,
        init_bodies,
        mut helper_reserved_atoms,
        ..
    } = plan;
    let body_items: Vec<ModuleItem> = external_import_bindings
        .iter()
        .filter_map(|binding| external_imports.get(binding))
        .map(make_external_import_stmt)
        .chain(
            named_imports
                .iter()
                .map(|(specifier, names)| make_named_import_stmt(names, specifier)),
        )
        .chain(owned_items.iter().filter_map(|(index, owned_atoms)| {
            filter_item_to_owned_bindings(&source_items[*index], owned_atoms)
        }))
        .chain(export_items(&export_names))
        .collect();
    let extra_code = emit_items(body_items, module.filename.clone(), cm.clone(), positions);
    module.code.push('\n');
    module.append_mapped(extra_code);
    for (name, cjs_params, stmts) in init_bodies {
        module.append_mapped(emit_factory_function_code(
            &name,
            cjs_params.as_ref(),
            stmts,
            &mut helper_reserved_atoms,
            module.filename.clone(),
            cm.clone(),
            positions,
        ));
    }
}

/// Re-synthesize a demoted lazy factory into entry-resident items: a guard
/// flag plus a plain init function, mirroring [`emit_esm_init_function_code`]
/// without the export. The helper wrapper (`__esm`) was already stripped from
/// the entry, so the source statement cannot simply be restored. The function
/// keeps the factory statement's span so entry provenance still covers the
/// original bytes.
fn synthesize_entry_init_items(
    var_name: &Atom,
    guard_name: &Atom,
    body_stmts: Vec<Stmt>,
    span: Span,
) -> Vec<(u32, ModuleItem)> {
    let guard = Ident::new(guard_name.clone(), DUMMY_SP, SyntaxContext::empty());
    let guard_decl = ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: guard.clone(),
                type_ann: None,
            }),
            init: Some(Box::new(Expr::Lit(Lit::Bool(Bool {
                span: DUMMY_SP,
                value: false,
            })))),
            definite: false,
        }],
    }))));

    let mut stmts: Vec<Stmt> = Vec::with_capacity(body_stmts.len() + 2);
    stmts.push(Stmt::If(IfStmt {
        span: DUMMY_SP,
        test: Box::new(Expr::Ident(guard.clone())),
        cons: Box::new(Stmt::Return(ReturnStmt {
            span: DUMMY_SP,
            arg: None,
        })),
        alt: None,
    }));
    stmts.push(Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent {
                id: guard,
                type_ann: None,
            })),
            right: Box::new(Expr::Lit(Lit::Bool(Bool {
                span: DUMMY_SP,
                value: true,
            }))),
        })),
    }));
    stmts.extend(body_stmts);

    let init_fn = ModuleItem::Stmt(Stmt::Decl(Decl::Fn(FnDecl {
        ident: Ident::new(var_name.clone(), DUMMY_SP, SyntaxContext::empty()),
        declare: false,
        function: Box::new(Function {
            params: Vec::new(),
            decorators: Vec::new(),
            span,
            ctxt: SyntaxContext::empty(),
            body: Some(FunctionBody {
                span: DUMMY_SP,
                stmts,
            }),
            is_generator: false,
            is_async: false,
            type_params: None,
            return_type: None,
            this_param: None,
        }),
    })));

    vec![(span.lo.0, guard_decl), (span.lo.0, init_fn)]
}

/// Re-synthesize a demoted CommonJS factory into the entry as a cached
/// callable, mirroring the standalone emission shape:
/// `var cache; function name() { if (cache) return ...; var exports = {};
/// [var module = { exports };] cache = ...; <body> return ...; }`.
fn synthesize_entry_cjs_items(
    var_name: &Atom,
    cache_name: &Atom,
    cjs_params: &CjsFactoryParams,
    body_stmts: Vec<Stmt>,
    span: Span,
) -> Vec<(u32, ModuleItem)> {
    let ident = |sym: &Atom| Ident::new(sym.clone(), DUMMY_SP, SyntaxContext::empty());
    let var_stmt = |name: Ident, init: Option<Expr>| {
        Stmt::Decl(Decl::Var(Box::new(VarDecl {
            span: DUMMY_SP,
            ctxt: SyntaxContext::empty(),
            kind: VarDeclKind::Var,
            declare: false,
            decls: vec![VarDeclarator {
                span: DUMMY_SP,
                name: Pat::Ident(BindingIdent {
                    id: name,
                    type_ann: None,
                }),
                init: init.map(Box::new),
                definite: false,
            }],
        })))
    };
    let exports_member = |obj: Ident| {
        Expr::Member(MemberExpr {
            span: DUMMY_SP,
            obj: Box::new(Expr::Ident(obj)),
            prop: MemberProp::Ident(IdentName::new("exports".into(), DUMMY_SP)),
        })
    };

    let cache = ident(cache_name);
    let exports = ident(&cjs_params.exports);
    let module = cjs_params.module.as_ref().map(ident);
    let cache_decl = ModuleItem::Stmt(var_stmt(cache.clone(), None));

    let cached_return = match &module {
        Some(_) => exports_member(cache.clone()),
        None => Expr::Ident(cache.clone()),
    };
    let mut stmts: Vec<Stmt> = Vec::with_capacity(body_stmts.len() + 5);
    stmts.push(Stmt::If(IfStmt {
        span: DUMMY_SP,
        test: Box::new(Expr::Ident(cache.clone())),
        cons: Box::new(Stmt::Return(ReturnStmt {
            span: DUMMY_SP,
            arg: Some(Box::new(cached_return)),
        })),
        alt: None,
    }));
    stmts.push(var_stmt(
        exports.clone(),
        Some(Expr::Object(ObjectLit {
            span: DUMMY_SP,
            props: Vec::new(),
        })),
    ));
    if let Some(module) = &module {
        stmts.push(var_stmt(
            module.clone(),
            Some(Expr::Object(ObjectLit {
                span: DUMMY_SP,
                props: vec![PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                    key: PropName::Ident(IdentName::new("exports".into(), DUMMY_SP)),
                    value: Box::new(Expr::Ident(exports.clone())),
                })))],
            })),
        ));
    }
    let cache_value = match &module {
        Some(module) => Expr::Ident(module.clone()),
        None => Expr::Ident(exports.clone()),
    };
    stmts.push(Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent {
                id: cache,
                type_ann: None,
            })),
            right: Box::new(cache_value),
        })),
    }));
    stmts.extend(body_stmts);
    let return_value = match &module {
        Some(module) => exports_member(module.clone()),
        None => Expr::Ident(exports),
    };
    stmts.push(Stmt::Return(ReturnStmt {
        span: DUMMY_SP,
        arg: Some(Box::new(return_value)),
    }));

    let factory_fn = ModuleItem::Stmt(Stmt::Decl(Decl::Fn(FnDecl {
        ident: Ident::new(var_name.clone(), DUMMY_SP, SyntaxContext::empty()),
        declare: false,
        function: Box::new(Function {
            params: Vec::new(),
            decorators: Vec::new(),
            span,
            ctxt: SyntaxContext::empty(),
            body: Some(FunctionBody {
                span: DUMMY_SP,
                stmts,
            }),
            is_generator: false,
            is_async: false,
            type_params: None,
            return_type: None,
            this_param: None,
        }),
    })));

    vec![(span.lo.0, cache_decl), (span.lo.0, factory_fn)]
}

/// Every identifier name mentioned in `stmts`, at any depth: declarations,
/// parameters, and references alike.
fn ident_atoms_in_stmts(stmts: &[Stmt]) -> HashSet<Atom> {
    struct IdentAtomCollector {
        atoms: HashSet<Atom>,
    }
    impl Visit for IdentAtomCollector {
        fn visit_ident(&mut self, ident: &Ident) {
            self.atoms.insert(ident.sym.clone());
        }
    }
    let mut collector = IdentAtomCollector {
        atoms: HashSet::default(),
    };
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.atoms
}

/// Emit a factory as an exported callable: a CommonJS factory becomes a
/// cached `require`-style function, a lazy ESM initializer a guarded init
/// function. The cache or guard takes a name not already reserved in the
/// target module and adds it to `reserved`.
fn emit_factory_function_code(
    name: &Atom,
    cjs_params: Option<&CjsFactoryParams>,
    stmts: Vec<Stmt>,
    reserved: &mut HashSet<Atom>,
    filename: String,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> MappedCode {
    // The helper is read inside the callable, so a body local, parameter, or
    // free reference with the same name would shadow it. Reserve every
    // identifier the body mentions; over-reserving only costs a suffix.
    reserved.extend(ident_atoms_in_stmts(&stmts));
    let body = emit_items(
        stmts.into_iter().map(ModuleItem::Stmt).collect(),
        filename,
        cm,
        positions,
    );
    let mut code = MappedCode::default();
    let Some(cjs_params) = cjs_params else {
        let guard = reserve_import_atom(&format!("__wakaru_{name}_initialized").into(), reserved);
        code.push_str(&format!(
            "var {guard} = false;\nexport function {name}() {{\nif ({guard}) return;\n{guard} = true;\n"
        ));
        code.push_mapped(body);
        code.push_str("\n}\n");
        return code;
    };
    let cache = reserve_import_atom(&format!("__wakaru_{name}_cache").into(), reserved);
    let exports = &cjs_params.exports;
    code.push_str(&format!("var {cache};\nexport function {name}() {{\n"));
    match &cjs_params.module {
        Some(module) => {
            code.push_str(&format!("if ({cache}) return {cache}.exports;\n"));
            code.push_str(&format!("var {exports} = {{}};\n"));
            code.push_str(&format!("var {module} = {{ exports: {exports} }};\n"));
            code.push_str(&format!("{cache} = {module};\n"));
            code.push_mapped(body);
            code.push_str(&format!("\nreturn {module}.exports;\n}}\n"));
        }
        None => {
            code.push_str(&format!("if ({cache}) return {cache};\n"));
            code.push_str(&format!("var {exports} = {{}};\n"));
            code.push_str(&format!("{cache} = {exports};\n"));
            code.push_mapped(body);
            code.push_str(&format!("\nreturn {exports};\n}}\n"));
        }
    }
    code
}

pub(super) fn repair_entry_imports(
    entry_items: Vec<ModuleItem>,
    binding_to_filename: &HashMap<BindingId, String>,
) -> Vec<ModuleItem> {
    repair_module_imports(entry_items, "entry.js", binding_to_filename)
}

pub(super) fn repair_module_imports(
    mut entry_items: Vec<ModuleItem>,
    current_filename: &str,
    binding_to_filename: &HashMap<BindingId, String>,
) -> Vec<ModuleItem> {
    let candidate_atoms: HashSet<Atom> = binding_to_filename
        .iter()
        .filter(|(_, filename)| filename.as_str() != current_filename)
        .map(|((atom, _), _)| atom.clone())
        .collect();
    if candidate_atoms.is_empty() {
        return entry_items;
    }

    let mut collector = AtomRefCollector {
        candidate_atoms: &candidate_atoms,
        references: HashSet::default(),
        shadowed_atoms: vec![HashSet::default()],
    };
    for item in &entry_items {
        item.visit_with(&mut collector);
    }

    let mut already_imported: HashSet<Atom> = entry_items
        .iter()
        .flat_map(|item| {
            module_item_import_binding_ids(item)
                .into_iter()
                .chain(module_item_declared_binding_ids(item))
        })
        .map(|(atom, _)| atom)
        .collect();
    let binding_filename_by_atom = atom_to_filename_binding_map(binding_to_filename);
    let mut imports_by_source: HashMap<String, Vec<Atom>> = HashMap::default();
    for atom in collector.references {
        if already_imported.contains(&atom) {
            continue;
        }
        let Some((_, source_filename)) = binding_filename_by_atom.get(&atom) else {
            continue;
        };
        if source_filename == current_filename {
            continue;
        }
        let specifier = relative_import_path(current_filename, source_filename);
        imports_by_source
            .entry(specifier)
            .or_default()
            .push(atom.clone());
        already_imported.insert(atom);
    }

    if imports_by_source.is_empty() {
        return entry_items;
    }

    let mut import_items = Vec::new();
    let mut sources: Vec<String> = imports_by_source.keys().cloned().collect();
    sources.sort();
    for source in sources {
        let names = imports_by_source.get_mut(&source).unwrap();
        names.sort();
        names.dedup();
        import_items.push(make_named_import_stmt(names, &source));
    }
    import_items.append(&mut entry_items);
    import_items
}
