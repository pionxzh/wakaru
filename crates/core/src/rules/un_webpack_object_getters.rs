use std::collections::{HashMap, HashSet};

use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    BlockStmt, BlockStmtOrExpr, Bool, Callee, Decl, Expr, ExprStmt, FnExpr, GetterProp, Ident, Lit,
    MemberProp, Module, ModuleDecl, ModuleItem, ObjectLit, Pat, Prop, PropName, PropOrSpread,
    ReturnStmt, Stmt, Str, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::decl_utils::BindingId;

pub struct UnWebpackObjectGetters {
    unresolved_mark: Mark,
}

impl UnWebpackObjectGetters {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for UnWebpackObjectGetters {
    fn visit_mut_module(&mut self, module: &mut Module) {
        module.visit_mut_children_with(self);
        rewrite_module_items(&mut module.body, self.unresolved_mark);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        rewrite_stmts(stmts);
    }
}

fn rewrite_module_items(items: &mut Vec<ModuleItem>, unresolved_mark: Mark) {
    let mut original = std::mem::take(items);
    let (mut replacements, removed) = plan_webpack_namespace_rewrites(&original, unresolved_mark);
    let mut rewritten = Vec::with_capacity(original.len());
    let mut i = 0;

    while i < original.len() {
        if removed.contains(&i) {
            i += 1;
            continue;
        }
        if let Some(item) = replacements.remove(&i) {
            rewritten.push(item);
            i += 1;
            continue;
        }

        if i + 1 < original.len() {
            if let Some(item) = maybe_rewrite_module_item_pair(&original[i], &original[i + 1]) {
                rewritten.push(item);
                i += 2;
                continue;
            }
        }

        rewritten.push(std::mem::replace(
            &mut original[i],
            ModuleItem::Stmt(Stmt::Empty(swc_core::ecma::ast::EmptyStmt {
                span: DUMMY_SP,
            })),
        ));
        i += 1;
    }

    *items = rewritten;
}

fn plan_webpack_namespace_rewrites(
    items: &[ModuleItem],
    unresolved_mark: Mark,
) -> (HashMap<usize, ModuleItem>, HashSet<usize>) {
    let mut replacements = HashMap::new();
    let mut removed = HashSet::new();

    for index in 0..items.len() {
        if removed.contains(&index) {
            continue;
        }
        let Some(binding) = extract_empty_object_binding_from_module_item(&items[index]) else {
            continue;
        };
        let Some((replacement, remove_indices)) =
            maybe_rewrite_webpack_namespace(items, index, &binding, unresolved_mark)
        else {
            continue;
        };

        replacements.insert(index, replacement);
        removed.extend(remove_indices);
    }

    (replacements, removed)
}

fn maybe_rewrite_webpack_namespace(
    items: &[ModuleItem],
    decl_index: usize,
    target: &BindingId,
    unresolved_mark: Mark,
) -> Option<(ModuleItem, Vec<usize>)> {
    let mut require_r_indices = Vec::new();
    let mut odp_getters = Vec::new();
    let mut odp_indices = Vec::new();
    let mut index = decl_index + 1;

    while index < items.len() {
        if is_require_r_module_item(&items[index], target, unresolved_mark) {
            require_r_indices.push(index);
            index += 1;
            continue;
        }

        if let Some(getters) =
            extract_require_d_map_getters_module_item(&items[index], target, unresolved_mark)
        {
            if require_r_indices.is_empty() || getters.len() < 2 {
                return None;
            }
            let mut replacement = items[decl_index].clone();
            replace_module_item_init_with_getters(&mut replacement, getters)?;
            require_r_indices.push(index);
            return Some((replacement, require_r_indices));
        }

        // Collect consecutive Object.defineProperty(target, name, { get: ... }) calls.
        if let Some(getter) = extract_single_define_property_getter(&items[index], target) {
            odp_getters.push(getter);
            odp_indices.push(index);
            index += 1;
            continue;
        }

        if module_item_references_binding(&items[index], target) {
            break;
        }
        index += 1;
    }

    if odp_getters.len() >= 2 {
        let mut replacement = items[decl_index].clone();
        replace_module_item_init_with_getters(&mut replacement, odp_getters)?;
        let mut removed = require_r_indices;
        removed.extend(odp_indices);
        return Some((replacement, removed));
    }

    None
}

fn rewrite_stmts(stmts: &mut Vec<Stmt>) {
    let mut original = std::mem::take(stmts);
    let mut rewritten = Vec::with_capacity(original.len());
    let mut skip_until = 0;
    let mut i = 0;

    while i < original.len() {
        if i < skip_until {
            i += 1;
            continue;
        }

        if i + 1 < original.len() {
            if let Some(stmt) = maybe_rewrite_stmt_pair(&original[i], &original[i + 1]) {
                rewritten.push(stmt);
                i += 2;
                continue;
            }
        }

        if let Some(binding) = extract_empty_object_binding_from_stmt(&original[i]) {
            let mut getters = Vec::new();
            let mut j = i + 1;
            while j < original.len() {
                if let Some(getter) =
                    extract_single_define_property_getter_from_stmt(&original[j], &binding)
                {
                    getters.push(getter);
                    j += 1;
                } else {
                    break;
                }
            }
            if getters.len() >= 2 {
                if let Some(stmt) = rewrite_stmt_init_with_getters(original[i].clone(), getters) {
                    rewritten.push(stmt);
                    skip_until = j;
                    i = j;
                    continue;
                }
            }
        }

        rewritten.push(std::mem::replace(
            &mut original[i],
            Stmt::Empty(swc_core::ecma::ast::EmptyStmt { span: DUMMY_SP }),
        ));
        i += 1;
    }

    *stmts = rewritten;
}

fn maybe_rewrite_module_item_pair(current: &ModuleItem, next: &ModuleItem) -> Option<ModuleItem> {
    let binding = extract_empty_object_binding_from_module_item(current)?;
    let ModuleItem::Stmt(next_stmt) = next else {
        return None;
    };
    let getters = extract_define_properties_getters(next_stmt, &binding)?;
    if getters.len() < 2 {
        return None;
    }

    match current {
        ModuleItem::Stmt(stmt) => Some(ModuleItem::Stmt(rewrite_stmt_init_with_getters(
            stmt.clone(),
            getters,
        )?)),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
            let mut export_decl = export_decl.clone();
            let Decl::Var(var_decl) = &mut export_decl.decl else {
                return None;
            };
            replace_var_decl_init_with_getters(&mut var_decl.decls, getters)?;
            Some(ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)))
        }
        _ => None,
    }
}

fn replace_module_item_init_with_getters(
    item: &mut ModuleItem,
    getters: Vec<GetterProp>,
) -> Option<()> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
            replace_var_decl_init_with_getters(&mut var_decl.decls, getters)
        }
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
            let Decl::Var(var_decl) = &mut export_decl.decl else {
                return None;
            };
            replace_var_decl_init_with_getters(&mut var_decl.decls, getters)
        }
        _ => None,
    }
}

fn maybe_rewrite_stmt_pair(current: &Stmt, next: &Stmt) -> Option<Stmt> {
    let binding = extract_empty_object_binding_from_stmt(current)?;
    let getters = extract_define_properties_getters(next, &binding)?;
    if getters.len() < 2 {
        return None;
    }

    rewrite_stmt_init_with_getters(current.clone(), getters)
}

fn rewrite_stmt_init_with_getters(stmt: Stmt, getters: Vec<GetterProp>) -> Option<Stmt> {
    let Stmt::Decl(Decl::Var(mut var_decl)) = stmt else {
        return None;
    };
    replace_var_decl_init_with_getters(&mut var_decl.decls, getters)?;
    Some(Stmt::Decl(Decl::Var(var_decl)))
}

fn replace_var_decl_init_with_getters(
    decls: &mut [VarDeclarator],
    getters: Vec<GetterProp>,
) -> Option<()> {
    if decls.len() != 1 {
        return None;
    }
    let decl = &mut decls[0];
    let Pat::Ident(_) = &decl.name else {
        return None;
    };

    decl.init = Some(Box::new(Expr::Object(ObjectLit {
        span: DUMMY_SP,
        props: getters
            .into_iter()
            .map(|getter| PropOrSpread::Prop(Box::new(Prop::Getter(getter))))
            .collect(),
    })));

    Some(())
}

fn extract_empty_object_binding_from_module_item(item: &ModuleItem) -> Option<BindingId> {
    match item {
        ModuleItem::Stmt(stmt) => extract_empty_object_binding_from_stmt(stmt),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
            let Decl::Var(var_decl) = &export_decl.decl else {
                return None;
            };
            extract_empty_object_binding_from_var_decl(&var_decl.decls)
        }
        _ => None,
    }
}

fn extract_empty_object_binding_from_stmt(stmt: &Stmt) -> Option<BindingId> {
    let Stmt::Decl(Decl::Var(var_decl)) = stmt else {
        return None;
    };
    extract_empty_object_binding_from_var_decl(&var_decl.decls)
}

fn extract_empty_object_binding_from_var_decl(decls: &[VarDeclarator]) -> Option<BindingId> {
    if decls.len() != 1 {
        return None;
    }
    let decl = &decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let Expr::Object(obj) = decl.init.as_deref()? else {
        return None;
    };
    if !obj.props.is_empty() {
        return None;
    }
    Some((binding.id.sym.clone(), binding.id.ctxt))
}

/// Extract a getter from `Object.defineProperty(target, "name", { enumerable: true, get: ... })`.
fn extract_single_define_property_getter(
    item: &ModuleItem,
    target: &BindingId,
) -> Option<GetterProp> {
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
        return None;
    };
    extract_single_define_property_getter_from_expr(expr.as_ref(), target)
}

fn extract_single_define_property_getter_from_stmt(
    stmt: &Stmt,
    target: &BindingId,
) -> Option<GetterProp> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    extract_single_define_property_getter_from_expr(expr.as_ref(), target)
}

fn extract_single_define_property_getter_from_expr(
    expr: &Expr,
    target: &BindingId,
) -> Option<GetterProp> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    let Expr::Ident(object_ident) = member.obj.as_ref() else {
        return None;
    };
    if object_ident.sym.as_ref() != "Object" {
        return None;
    }
    let MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    if prop.sym.as_ref() != "defineProperty" || call.args.len() != 3 {
        return None;
    }
    if call.args.iter().any(|a| a.spread.is_some()) {
        return None;
    }

    let Expr::Ident(target_ident) = call.args[0].expr.as_ref() else {
        return None;
    };
    if target_ident.sym != target.0 || target_ident.ctxt != target.1 {
        return None;
    }

    let prop_name = match call.args[1].expr.as_ref() {
        Expr::Lit(Lit::Str(s)) => str_to_prop_name(s),
        _ => return None,
    };

    extract_getter_descriptor(&prop_name, call.args[2].expr.as_ref())
}

fn extract_define_properties_getters(stmt: &Stmt, target: &BindingId) -> Option<Vec<GetterProp>> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    let Expr::Ident(object_ident) = member.obj.as_ref() else {
        return None;
    };
    if object_ident.sym.as_ref() != "Object" {
        return None;
    }
    let MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    if prop.sym.as_ref() != "defineProperties" || call.args.len() != 2 {
        return None;
    }

    let Expr::Ident(target_ident) = call.args[0].expr.as_ref() else {
        return None;
    };
    if target_ident.sym != target.0 || target_ident.ctxt != target.1 {
        return None;
    }

    let Expr::Object(descriptor_map) = call.args[1].expr.as_ref() else {
        return None;
    };

    let mut getters = Vec::with_capacity(descriptor_map.props.len());
    for prop in &descriptor_map.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(entry) = prop.as_ref() else {
            return None;
        };
        getters.push(extract_getter_descriptor(&entry.key, entry.value.as_ref())?);
    }

    Some(getters)
}

fn is_require_r_module_item(item: &ModuleItem, target: &BindingId, unresolved_mark: Mark) -> bool {
    let ModuleItem::Stmt(stmt) = item else {
        return false;
    };
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return false;
    };
    if !is_require_member_call(call, "r", unresolved_mark) || call.args.len() != 1 {
        return false;
    }
    matches!(call.args[0].expr.as_ref(), Expr::Ident(id) if id.sym == target.0 && id.ctxt == target.1)
}

fn extract_require_d_map_getters_module_item(
    item: &ModuleItem,
    target: &BindingId,
    unresolved_mark: Mark,
) -> Option<Vec<GetterProp>> {
    let ModuleItem::Stmt(stmt) = item else {
        return None;
    };
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    if !is_require_member_call(call, "d", unresolved_mark) || call.args.len() != 2 {
        return None;
    }
    let Expr::Ident(target_ident) = call.args[0].expr.as_ref() else {
        return None;
    };
    if target_ident.sym != target.0 || target_ident.ctxt != target.1 {
        return None;
    }
    let Expr::Object(getter_map) = call.args[1].expr.as_ref() else {
        return None;
    };

    let mut getters = Vec::with_capacity(getter_map.props.len());
    let mut seen = HashSet::new();
    for prop in &getter_map.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let getter = match prop.as_ref() {
            Prop::Method(method) => {
                let name = prop_name_as_str(&method.key)?;
                if !seen.insert(name.to_string())
                    || !method.function.params.is_empty()
                    || method.function.is_async
                    || method.function.is_generator
                {
                    return None;
                }
                let body = method.function.body.clone()?;
                GetterProp {
                    span: DUMMY_SP,
                    key: method.key.clone(),
                    type_ann: None,
                    body: Some(body),
                }
            }
            Prop::KeyValue(entry) => {
                let name = prop_name_as_str(&entry.key)?;
                if !seen.insert(name.to_string()) {
                    return None;
                }
                GetterProp {
                    span: DUMMY_SP,
                    key: entry.key.clone(),
                    type_ann: None,
                    body: Some(extract_getter_body(entry.value.as_ref())?),
                }
            }
            _ => return None,
        };
        getters.push(getter);
    }

    Some(getters)
}

fn is_require_member_call(
    call: &swc_core::ecma::ast::CallExpr,
    prop_name: &str,
    unresolved_mark: Mark,
) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    let Expr::Ident(require_ident) = member.obj.as_ref() else {
        return false;
    };
    if require_ident.sym.as_ref() != "require" || require_ident.ctxt.outer() != unresolved_mark {
        return false;
    }
    matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == prop_name)
}

fn module_item_references_binding(item: &ModuleItem, target: &BindingId) -> bool {
    let mut finder = BindingRefFinder {
        target,
        found: false,
    };
    item.visit_with(&mut finder);
    finder.found
}

struct BindingRefFinder<'a> {
    target: &'a BindingId,
    found: bool,
}

impl Visit for BindingRefFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == self.target.0 && ident.ctxt == self.target.1 {
            self.found = true;
        }
    }
}

fn extract_getter_descriptor(key: &PropName, descriptor: &Expr) -> Option<GetterProp> {
    let Expr::Object(object) = descriptor else {
        return None;
    };

    let mut enumerable_true = false;
    let mut getter_body = None;

    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(entry) = prop.as_ref() else {
            return None;
        };

        match prop_name_as_str(&entry.key)? {
            "enumerable" => {
                let Expr::Lit(Lit::Bool(Bool { value: true, .. })) = entry.value.as_ref() else {
                    return None;
                };
                enumerable_true = true;
            }
            "get" => {
                getter_body = Some(extract_getter_body(entry.value.as_ref())?);
            }
            _ => return None,
        }
    }

    if !enumerable_true {
        return None;
    }

    Some(GetterProp {
        span: DUMMY_SP,
        key: key.clone(),
        type_ann: None,
        body: Some(getter_body?),
    })
}

fn extract_getter_body(expr: &Expr) -> Option<BlockStmt> {
    match expr {
        Expr::Fn(FnExpr { ident, function }) => {
            if ident.is_some()
                || !function.params.is_empty()
                || function.is_async
                || function.is_generator
            {
                return None;
            }
            function.body.clone()
        }
        Expr::Arrow(arrow) => {
            if !arrow.params.is_empty() || arrow.is_async || arrow.is_generator {
                return None;
            }
            match arrow.body.as_ref() {
                BlockStmtOrExpr::BlockStmt(block) => Some(block.clone()),
                BlockStmtOrExpr::Expr(expr) => Some(BlockStmt {
                    span: DUMMY_SP,
                    ctxt: arrow.ctxt,
                    stmts: vec![Stmt::Return(ReturnStmt {
                        span: DUMMY_SP,
                        arg: Some(expr.clone()),
                    })],
                }),
            }
        }
        _ => None,
    }
}

fn str_to_prop_name(s: &Str) -> PropName {
    match s.value.as_str() {
        Some(value) if is_valid_ident(value) => PropName::Ident(value.into()),
        _ => PropName::Str(s.clone()),
    }
}

fn is_valid_ident(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' && first != '$' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

fn prop_name_as_str(name: &PropName) -> Option<&str> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.as_ref()),
        PropName::Str(value) => value.value.as_str(),
        _ => None,
    }
}
