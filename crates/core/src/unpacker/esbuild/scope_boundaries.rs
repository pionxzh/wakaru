//! `__export(namespace, { ... })` boundary detection for scope-hoisted code.

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    ArrowFunctionBody, Callee, Decl, Expr, ExprStmt, ForInStmt, MemberProp, ModuleDecl, ModuleItem,
    ObjectLit, Pat, PropName, Stmt, VarDeclarator,
};

use crate::collections::{HashMap, HashSet};
use crate::unpacker::BindingId;

use super::bindings::ItemBindingInfo;
use super::synthesis::pat_declared_binding_ids;

pub(super) struct ScopeHoistedBoundary {
    pub(super) ns_atom: Atom,
    pub(super) ns_binding: BindingId,
    pub(super) ns_decl_index: usize,
    pub(super) export_call_index: usize,
    pub(super) export_entries: Vec<(Atom, BindingId)>,
    pub(super) exported_bindings: HashSet<BindingId>,
}

/// Detect the `__export` helper: an arrow with 2 params whose body is a
/// single for-in loop (iterating over the second param).
pub(super) fn detect_export_helper(items: &[ModuleItem]) -> Option<(usize, BindingId)> {
    for (index, item) in items.iter().enumerate() {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(bi) = &decl.name else { continue };
            let Some(init) = &decl.init else { continue };
            if is_export_helper(init) {
                return Some((index, (bi.id.sym.clone(), bi.id.ctxt)));
            }
        }
    }
    None
}

/// Check if an expression matches the __export pattern:
///   (target, all) => { for (var name in all) defProp(...) }
fn is_export_helper(expr: &Expr) -> bool {
    let Expr::Arrow(arrow) = expr else {
        return false;
    };
    if arrow.params.len() != 2 {
        return false;
    }
    let ArrowFunctionBody::FunctionBody(block) = &*arrow.body else {
        return false;
    };
    if block.stmts.len() != 1 {
        return false;
    }
    matches!(&block.stmts[0], Stmt::ForIn(ForInStmt { right, .. })
        if matches!(&**right, Expr::Ident(id) if same_param_ident(&arrow.params[1], &id.sym)))
}

fn same_param_ident(pat: &Pat, sym: &Atom) -> bool {
    matches!(pat, Pat::Ident(bi) if bi.id.sym == *sym)
}

/// Find all namespace + __export call pairs.
/// Pattern: `var NS = {};` at index i, `__export(NS, { ... })` at index i+1.
pub(super) fn collect_scope_hoisted_boundaries(
    items: &[ModuleItem],
    export_helper: &BindingId,
) -> Vec<ScopeHoistedBoundary> {
    let mut boundaries = Vec::new();

    for i in 0..items.len().saturating_sub(1) {
        // Check: var NS = {};
        let Some(ns_binding) = extract_empty_object_decl(&items[i]) else {
            continue;
        };

        // Check: __export(NS, { ... }) at i+1
        if !is_export_call(&items[i + 1], export_helper, &ns_binding) {
            continue;
        }

        let export_entries = extract_export_entries(&items[i + 1]);
        let exported_bindings = export_entries
            .iter()
            .map(|(_, binding)| binding.clone())
            .collect();

        boundaries.push(ScopeHoistedBoundary {
            ns_atom: ns_binding.0.clone(),
            ns_binding,
            ns_decl_index: i,
            export_call_index: i + 1,
            export_entries,
            exported_bindings,
        });
    }

    boundaries
}

/// Check if a namespace atom appears in any ESM export declaration.
/// e.g. `export { math_exports as math }` contains the ident `math_exports`.
pub(super) fn namespace_is_module_exported(
    items: &[ModuleItem],
    item_infos: &[ItemBindingInfo],
    ns_binding: &BindingId,
) -> bool {
    items.iter().enumerate().any(|(i, item)| {
        matches!(item, ModuleItem::ModuleDecl(_))
            && item_infos
                .get(i)
                .is_some_and(|info| info.references.contains(ns_binding))
    })
}

/// Extract the binding atoms from `__export(NS, { key: () => binding, ... })`.
fn extract_export_entries(item: &ModuleItem) -> Vec<(Atom, BindingId)> {
    let mut entries = Vec::new();
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
        return entries;
    };
    let Expr::Call(call) = &**expr else {
        return entries;
    };
    if call.args.len() != 2 {
        return entries;
    }
    let Expr::Object(obj) = &*call.args[1].expr else {
        return entries;
    };
    for prop in &obj.props {
        let swc_core::ecma::ast::PropOrSpread::Prop(prop) = prop else {
            continue;
        };
        let swc_core::ecma::ast::Prop::KeyValue(kv) = &**prop else {
            continue;
        };
        let Some(export_name) = prop_name_atom(&kv.key) else {
            continue;
        };
        // Value is `() => binding` — extract the binding ident from the arrow body.
        let Expr::Arrow(arrow) = &*kv.value else {
            continue;
        };
        if let ArrowFunctionBody::Expr(body_expr) = &*arrow.body {
            if let Expr::Ident(id) = &**body_expr {
                entries.push((export_name, (id.sym.clone(), id.ctxt)));
            }
        }
    }
    entries
}

fn prop_name_atom(name: &PropName) -> Option<Atom> {
    match name {
        PropName::Ident(id) => Some(id.sym.clone()),
        PropName::Str(s) => Some(s.value.as_str().unwrap_or("").to_string().into()),
        _ => None,
    }
}

/// Extract the binding from `var X = {};` (single declarator, empty object init).
fn extract_empty_object_decl(item: &ModuleItem) -> Option<BindingId> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(bi) = &decl.name else {
        return None;
    };
    let Some(init) = &decl.init else {
        return None;
    };
    let Expr::Object(ObjectLit { props, .. }) = &**init else {
        return None;
    };
    if !props.is_empty() {
        return None;
    }
    Some((bi.id.sym.clone(), bi.id.ctxt))
}

/// Check if an item is `__export(NS, { ... })`.
fn is_export_call(item: &ModuleItem, export_helper: &BindingId, ns_binding: &BindingId) -> bool {
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
        return false;
    };
    let Expr::Call(call) = &**expr else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(callee_id) = &**callee else {
        return false;
    };
    if callee_id.sym != export_helper.0 || callee_id.ctxt != export_helper.1 || call.args.len() != 2
    {
        return false;
    }
    // First arg must be the namespace ident.
    let Expr::Ident(first_arg) = &*call.args[0].expr else {
        return false;
    };
    if first_arg.sym != ns_binding.0 || first_arg.ctxt != ns_binding.1 {
        return false;
    }
    // Second arg must be an object literal (the export map).
    matches!(&*call.args[1].expr, Expr::Object(_))
}

/// Find the end index for the last scope-hoisted module.
///
/// Three-phase scan from `from`:
///   Phase 1: find the last item that declares an exported binding.
///            Everything up to it (inclusive) is module code — this
///            captures private helpers that precede exported declarations.
///   Phase 2: reference closure — extend to include declarations of names
///            referenced by the module code (private helpers after exports).
///   Phase 3: include trailing expression statements that reference module
///            bindings (side effects). Stop at unreferenced expressions,
///            declarations, or ModuleDecls.
pub(super) fn find_last_module_end(
    items: &[ModuleItem],
    item_infos: &[ItemBindingInfo],
    from: usize,
    exported_bindings: &HashSet<BindingId>,
    factory_referenced_atoms: &HashSet<Atom>,
) -> usize {
    // Phase 1: find the last item that declares an exported binding.
    let mut last_export_idx = None;
    for (i, item) in items.iter().enumerate().skip(from) {
        if is_module_boundary_item(item) {
            break;
        }
        if item_infos[i]
            .declared
            .iter()
            .any(|binding| exported_bindings.contains(binding))
        {
            last_export_idx = Some(i);
        }
    }

    let Some(last) = last_export_idx else {
        return from;
    };

    // Phase 2: reference closure — include declarations whose names are
    // referenced by the module code collected so far OR by factory modules.
    // This captures private helpers that esbuild emits after the exported
    // functions, whether they are called by other scope-hoisted code or by
    // factory modules.
    let mut end = last + 1;
    let mut module_bindings: HashSet<BindingId> = exported_bindings.clone();
    while end < items.len() {
        let item = &items[end];
        if is_module_boundary_item(item) {
            break;
        }
        let declared = &item_infos[end].declared;
        if declared.is_empty() {
            break;
        };
        let referenced_by_module = declared
            .iter()
            .any(|binding| items_reference_binding(&item_infos[from..end], binding));
        let referenced_by_factory = declared
            .iter()
            .any(|(atom, _)| factory_referenced_atoms.contains(atom));
        if !referenced_by_module && !referenced_by_factory {
            break;
        }

        for binding in declared {
            module_bindings.insert(binding.clone());
        }
        end += 1;
    }

    // Phase 3: include trailing expression statements that reference any
    // binding from this module (side effects like `register("self", ...)`
    // or `console.log(value)`). Stop at expressions that only reference
    // globals/literals, declarations, or ModuleDecls.
    for (i, item) in items.iter().enumerate().skip(end) {
        match item {
            item if is_module_boundary_item(item) => return i,
            ModuleItem::Stmt(Stmt::Expr(_)) => {
                if !item_infos[i]
                    .references
                    .iter()
                    .any(|binding| module_bindings.contains(binding))
                {
                    return i;
                }
            }
            ModuleItem::Stmt(Stmt::Decl(_)) | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(_)) => {
                return i;
            }
            _ => return i,
        }
    }
    items.len()
}

fn is_module_boundary_item(item: &ModuleItem) -> bool {
    // `export var/function/class ...` can still belong to the current
    // scope-hoisted module; imports and re-export declarations start a
    // separate module boundary.
    matches!(item, ModuleItem::ModuleDecl(decl) if !matches!(decl, ModuleDecl::ExportDecl(_)))
}

fn items_reference_binding(item_infos: &[ItemBindingInfo], binding: &BindingId) -> bool {
    item_infos
        .iter()
        .any(|info| info.references.contains(binding))
}

pub(super) fn removable_export_helper_dependency_indices(
    export_helper_index: usize,
    items: &[ModuleItem],
    item_infos: &[ItemBindingInfo],
    boundaries: &[ScopeHoistedBoundary],
) -> HashSet<usize> {
    let mut binding_to_index = HashMap::default();
    for (index, info) in item_infos.iter().enumerate() {
        for binding in &info.declared {
            binding_to_index.entry(binding.clone()).or_insert(index);
        }
    }

    let mut closure = HashSet::default();
    let mut stack = vec![export_helper_index];
    while let Some(index) = stack.pop() {
        if !closure.insert(index) {
            continue;
        }
        for reference in &item_infos[index].references {
            if let Some(&decl_index) = binding_to_index.get(reference) {
                stack.push(decl_index);
            }
        }
    }

    let boundary_export_calls: HashSet<usize> = boundaries
        .iter()
        .map(|boundary| boundary.export_call_index)
        .collect();
    let mut removable: HashSet<usize> = closure
        .into_iter()
        .filter(|&index| is_removable_export_helper_dependency_item(&items[index]))
        .collect();
    // Removal ignores reads from other removed items only. An item that stays
    // (such as a helper a chunk also exports) keeps everything it reads.
    loop {
        let kept: Vec<usize> = removable
            .iter()
            .copied()
            .filter(|&index| {
                item_infos[index].declared.iter().any(|binding| {
                    item_infos.iter().enumerate().any(|(consumer_index, info)| {
                        !removable.contains(&consumer_index)
                            && !boundary_export_calls.contains(&consumer_index)
                            && info.references.contains(binding)
                    })
                })
            })
            .collect();
        if kept.is_empty() {
            break;
        }
        for index in kept {
            removable.remove(&index);
        }
    }
    removable
}

fn is_removable_export_helper_dependency_item(item: &ModuleItem) -> bool {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(_))) => true,
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => var
            .decls
            .iter()
            .all(is_removable_export_helper_dependency_var),
        _ => false,
    }
}

fn is_removable_export_helper_dependency_var(decl: &VarDeclarator) -> bool {
    let Some(init) = decl.init.as_deref() else {
        return true;
    };

    matches!(init, Expr::Fn(_) | Expr::Arrow(_))
        || is_object_destructure_from_object(&decl.name, init)
        || is_object_member_alias(init)
}

pub(super) fn is_scope_support_declaration_for_binding(
    item: &ModuleItem,
    binding: &BindingId,
) -> bool {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
            fn_decl.ident.sym == binding.0 && fn_decl.ident.ctxt == binding.1
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => var_decl.decls.iter().any(|decl| {
            pat_declared_binding_ids(&decl.name)
                .iter()
                .any(|decl_binding| decl_binding == binding)
                && decl
                    .init
                    .as_deref()
                    .is_some_and(|init| matches!(init, Expr::Fn(_) | Expr::Arrow(_)))
        }),
        _ => false,
    }
}

fn is_object_destructure_from_object(name: &Pat, init: &Expr) -> bool {
    matches!(name, Pat::Object(_)) && matches!(init, Expr::Ident(ident) if ident.sym == *"Object")
}

fn is_object_member_alias(init: &Expr) -> bool {
    let Expr::Member(member) = init else {
        return false;
    };
    matches!(member.obj.as_ref(), Expr::Ident(ident) if ident.sym == *"Object")
        && matches!(&member.prop, MemberProp::Ident(_))
}
