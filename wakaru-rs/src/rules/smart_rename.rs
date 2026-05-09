use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrayPat, ArrowExpr, AssignPatProp, BlockStmtOrExpr, CallExpr, Callee, ClassDecl, ClassExpr,
    Decl, ExportSpecifier, Expr, FnDecl, FnExpr, Function, Ident, ImportDecl, ImportSpecifier,
    JSXElementName, JSXMemberExpr, JSXObject, KeyValuePatProp, Lit, MemberExpr, MemberProp, Module,
    ModuleDecl, ModuleExportName, ModuleItem, ObjectPat, ObjectPatProp, Param, Pat, Prop, PropName,
    Stmt, VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::rename_utils::{
    collect_module_names, rename_bindings, rename_bindings_in_module, rename_causes_shadowing,
    BindingId, BindingRename,
};
use super::ObjShorthand;

pub struct SmartRename;

impl VisitMut for SmartRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        react_rename_module(module);
        destructuring_rename_module(module);
        member_init_rename_module(module);
        symbol_for_rename_module(module);
        module.visit_mut_children_with(self);
        // Runs once at the module level; uses (sym, ctxt) matching so nested
        // bindings are classified correctly without per-scope recursion.
        value_position_rename_module(module);
        jsx_component_alias_rename_module(module);
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        react_rename_function_body(func);
        destructuring_rename_function(func);
        member_init_rename_function(func);
        symbol_for_rename_function(func);
        func.visit_mut_children_with(self);
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        destructuring_rename_arrow(arrow);
        member_init_rename_arrow(arrow);
        symbol_for_rename_arrow(arrow);
        arrow.visit_mut_children_with(self);
    }
}

// ============================================================
// React hook renames
// ============================================================

const REACT_MINIFIED_THRESHOLD: usize = 2;

fn react_rename_module(module: &mut Module) {
    let all_names = collect_names_in_module(&module.body);
    let renames = collect_react_renames_from_module_items(&module.body, &all_names);
    if renames.is_empty() {
        return;
    }
    rename_bindings_in_module(module, &renames);
}

fn react_rename_function_body(func: &mut Function) {
    let Some(body) = &mut func.body else { return };
    let all_names = collect_names_in_stmts(&body.stmts);
    let renames = collect_react_renames_from_stmts(&body.stmts, &all_names);
    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut body.stmts, &renames);
}

fn collect_react_renames_from_module_items(
    body: &[ModuleItem],
    all_names: &HashSet<String>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();

    for item in body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
                collect_react_var_decl_renames(var_decl, &mut renames, &mut used_names);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                if let Decl::Var(var_decl) = &export_decl.decl {
                    collect_react_var_decl_renames(var_decl, &mut renames, &mut used_names);
                }
            }
            _ => {}
        }
    }

    renames
}

fn collect_react_renames_from_stmts(
    stmts: &[Stmt],
    all_names: &HashSet<String>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();

    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var_decl)) = stmt else {
            continue;
        };
        collect_react_var_decl_renames(var_decl, &mut renames, &mut used_names);
    }

    renames
}

fn collect_react_var_decl_renames(
    var_decl: &VarDecl,
    renames: &mut Vec<BindingRename>,
    used_names: &mut HashSet<String>,
) {
    for decl in &var_decl.decls {
        match &decl.name {
            Pat::Ident(binding) => {
                if let Some(init) = &decl.init {
                    if let Some(hook_name) = get_single_react_hook_call(init) {
                        let old_name = binding.id.sym.to_string();
                        if old_name.chars().count() > REACT_MINIFIED_THRESHOLD {
                            continue;
                        }

                        let new_name = match hook_name.as_str() {
                            "useRef" => format!("{}Ref", old_name),
                            "createContext" => pascal_case_first(&old_name) + "Context",
                            _ => continue,
                        };

                        if !used_names.contains(&new_name) || new_name == old_name {
                            used_names.insert(new_name.clone());
                            renames.push(BindingRename {
                                old: (binding.id.sym.clone(), binding.id.ctxt),
                                new: new_name.as_str().into(),
                            });
                        }
                    }
                }
            }
            Pat::Array(array_pat) => {
                if let Some(init) = &decl.init {
                    if let Some(hook_name) = get_single_react_hook_call(init) {
                        collect_array_pat_react_renames(array_pat, &hook_name, renames, used_names);
                    }
                }
            }
            _ => {}
        }
    }
}

fn collect_array_pat_react_renames(
    array_pat: &ArrayPat,
    hook_name: &str,
    renames: &mut Vec<BindingRename>,
    used_names: &mut HashSet<String>,
) {
    match hook_name {
        "useState" => {
            let state_name = get_array_elem_name(array_pat, 0);
            if let Some((setter_name, setter_id)) = get_array_elem_if_short(array_pat, 1) {
                let base = state_name.unwrap_or_else(|| setter_name.clone());
                let new_setter = format!("set{}", pascal_case_first(&base));
                if !used_names.contains(&new_setter) || new_setter == setter_name {
                    used_names.insert(new_setter.clone());
                    renames.push(BindingRename {
                        old: setter_id,
                        new: new_setter.as_str().into(),
                    });
                }
            }
        }
        "useReducer" => {
            if let Some((state_name, state_id)) = get_array_elem_if_short(array_pat, 0) {
                let new_state = format!("{}State", state_name);
                if !used_names.contains(&new_state) || new_state == state_name {
                    used_names.insert(new_state.clone());
                    renames.push(BindingRename {
                        old: state_id,
                        new: new_state.as_str().into(),
                    });
                }
            }
            if let Some((dispatch_name, dispatch_id)) = get_array_elem_if_short(array_pat, 1) {
                let new_dispatch = format!("{}Dispatch", dispatch_name);
                if !used_names.contains(&new_dispatch) || new_dispatch == dispatch_name {
                    used_names.insert(new_dispatch.clone());
                    renames.push(BindingRename {
                        old: dispatch_id,
                        new: new_dispatch.as_str().into(),
                    });
                }
            }
        }
        _ => {}
    }
}

/// Returns the hook name if `expr` is a call to a known React hook.
fn get_single_react_hook_call(expr: &Expr) -> Option<String> {
    let Expr::Call(CallExpr { callee, args, .. }) = expr else {
        return None;
    };
    let fn_name = match callee {
        Callee::Expr(e) => match e.as_ref() {
            Expr::Ident(id) => id.sym.to_string(),
            Expr::Member(m) => {
                if let MemberProp::Ident(i) = &m.prop {
                    i.sym.to_string()
                } else {
                    return None;
                }
            }
            _ => return None,
        },
        _ => return None,
    };

    let valid = match fn_name.as_str() {
        "useRef" | "createContext" => args.len() <= 1,
        "useState" => args.len() <= 1,
        "useReducer" => !args.is_empty() && args.len() <= 3,
        "forwardRef" => args.len() == 1,
        _ => false,
    };

    if valid {
        Some(fn_name)
    } else {
        None
    }
}

fn get_array_elem_name(array_pat: &ArrayPat, idx: usize) -> Option<String> {
    let Some(Some(Pat::Ident(bi))) = array_pat.elems.get(idx) else {
        return None;
    };
    Some(bi.id.sym.to_string())
}

fn get_array_elem_if_short(array_pat: &ArrayPat, idx: usize) -> Option<(String, BindingId)> {
    let Some(Some(Pat::Ident(bi))) = array_pat.elems.get(idx) else {
        return None;
    };
    let name = bi.id.sym.to_string();
    if name.chars().count() <= REACT_MINIFIED_THRESHOLD {
        Some((name, (bi.id.sym.clone(), bi.id.ctxt)))
    } else {
        None
    }
}

// ============================================================
// Destructuring shorthand renames
// ============================================================

fn destructuring_rename_module(module: &mut Module) {
    let all_names = collect_names_in_module(&module.body);
    let renames = collect_obj_pat_renames_from_module(&module.body, &all_names);
    if renames.is_empty() {
        return;
    }
    rename_bindings_in_module(module, &renames);
    let mut shorthand = ObjectPatShorthandConverter;
    module.visit_mut_with(&mut shorthand);
}

fn destructuring_rename_function(func: &mut Function) {
    let Some(body) = &func.body else { return };
    let mut all_names = collect_names_in_stmts(&body.stmts);
    for p in &func.params {
        collect_names_in_pat(&p.pat, &mut all_names);
    }

    // Collect renames from both params and body VarDecls
    let mut renames = collect_obj_pat_renames_from_params(&func.params, &all_names);
    let body_renames = collect_obj_pat_renames_from_stmts(&body.stmts, &all_names);
    renames.extend(body_renames);

    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut func.params, &renames);
    if let Some(body) = &mut func.body {
        rename_bindings(&mut body.stmts, &renames);
    }
    let mut shorthand = ObjectPatShorthandConverter;
    func.params
        .iter_mut()
        .for_each(|p| p.visit_mut_with(&mut shorthand));
    if let Some(body) = &mut func.body {
        body.visit_mut_with(&mut shorthand);
    }
}

fn destructuring_rename_arrow(arrow: &mut ArrowExpr) {
    let mut all_names = match arrow.body.as_ref() {
        BlockStmtOrExpr::BlockStmt(b) => collect_names_in_stmts(&b.stmts),
        BlockStmtOrExpr::Expr(e) => {
            let mut names = HashSet::new();
            collect_names_in_expr(e, &mut names);
            names
        }
    };
    // Include param names to avoid renaming into duplicates
    for p in &arrow.params {
        collect_names_in_pat(p, &mut all_names);
    }
    let mut renames = collect_obj_pat_renames_from_pats(&arrow.params, &all_names);
    if let BlockStmtOrExpr::BlockStmt(b) = arrow.body.as_ref() {
        renames.extend(collect_obj_pat_renames_from_stmts(&b.stmts, &all_names));
    }
    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut arrow.params, &renames);
    match arrow.body.as_mut() {
        BlockStmtOrExpr::BlockStmt(block) => {
            rename_bindings(&mut block.stmts, &renames);
            block.visit_mut_with(&mut ObjectPatShorthandConverter);
        }
        BlockStmtOrExpr::Expr(expr) => rename_bindings(expr, &renames),
    }
    let mut shorthand = ObjectPatShorthandConverter;
    arrow
        .params
        .iter_mut()
        .for_each(|p| p.visit_mut_with(&mut shorthand));
}

fn collect_obj_pat_renames_from_module(
    body: &[ModuleItem],
    all_names: &HashSet<String>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();

    for item in body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    collect_obj_pat_renames_from_pat(&decl.name, &mut renames, &mut used_names);
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ed)) => {
                if let Decl::Var(var) = &ed.decl {
                    for decl in &var.decls {
                        collect_obj_pat_renames_from_pat(&decl.name, &mut renames, &mut used_names);
                    }
                }
            }
            _ => {}
        }
    }

    renames
}

fn collect_obj_pat_renames_from_params(
    params: &[Param],
    all_names: &HashSet<String>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();
    for p in params {
        collect_obj_pat_renames_from_pat(&p.pat, &mut renames, &mut used_names);
    }
    renames
}

fn collect_obj_pat_renames_from_stmts(
    stmts: &[Stmt],
    all_names: &HashSet<String>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            collect_obj_pat_renames_from_pat(&decl.name, &mut renames, &mut used_names);
        }
    }
    renames
}

fn collect_obj_pat_renames_from_pats(
    params: &[Pat],
    all_names: &HashSet<String>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();
    for p in params {
        collect_obj_pat_renames_from_pat(p, &mut renames, &mut used_names);
    }
    renames
}

fn collect_obj_pat_renames_from_pat(
    pat: &Pat,
    renames: &mut Vec<BindingRename>,
    used_names: &mut HashSet<String>,
) {
    let Pat::Object(obj_pat) = pat else { return };
    for prop in &obj_pat.props {
        match prop {
            ObjectPatProp::KeyValue(kv) => {
                let key_str = match &kv.key {
                    PropName::Ident(i) => i.sym.to_string(),
                    PropName::Str(s) => s.value.as_str().map(|s| s.to_string()).unwrap_or_default(),
                    _ => continue,
                };
                // For non-identifier keys (e.g. "aria-current"), sanitize to
                // a valid identifier (e.g. "aria_current") instead of skipping.
                let target_name = if is_valid_js_ident(&key_str) {
                    key_str.clone()
                } else {
                    sanitize_to_ident(&key_str)
                };
                if target_name.is_empty() {
                    continue;
                }
                let alias = match extract_binding_from_pat(&kv.value) {
                    Some(id) => id,
                    None => continue,
                };
                if alias.0.as_ref().chars().count() > REACT_MINIFIED_THRESHOLD {
                    continue;
                }
                if alias.0.as_ref() == target_name {
                    continue;
                }
                let new_name = find_non_conflicting_name(&target_name, used_names);
                used_names.insert(new_name.clone());
                renames.push(BindingRename {
                    old: alias,
                    new: new_name.as_str().into(),
                });
            }
            ObjectPatProp::Rest(rest_pat) => {
                // `...d` where `d` is short → rename to `rest`
                let Some(alias) = extract_binding_from_pat(&rest_pat.arg) else {
                    continue;
                };
                if alias.0.as_ref().chars().count() > REACT_MINIFIED_THRESHOLD {
                    continue;
                }
                let new_name = find_non_conflicting_name("rest", used_names);
                if new_name == alias.0.as_ref() {
                    continue;
                }
                used_names.insert(new_name.clone());
                renames.push(BindingRename {
                    old: alias,
                    new: new_name.as_str().into(),
                });
            }
            ObjectPatProp::Assign(_) => {}
        }
    }
}

fn extract_binding_from_pat(pat: &Pat) -> Option<BindingId> {
    match pat {
        Pat::Ident(bi) => Some((bi.id.sym.clone(), bi.id.ctxt)),
        Pat::Assign(assign_pat) => extract_binding_from_pat(&assign_pat.left),
        _ => None,
    }
}

fn find_non_conflicting_name(base: &str, used_names: &HashSet<String>) -> String {
    let base = if is_reserved_keyword(base) {
        format!("_{}", base)
    } else {
        base.to_string()
    };

    if !used_names.contains(&base) {
        return base;
    }
    for i in 1.. {
        let candidate = format!("{}_{}", base, i);
        if !used_names.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn is_reserved_keyword(name: &str) -> bool {
    matches!(
        name,
        "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "let"
            | "new"
            | "null"
            | "return"
            | "static"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
            | "enum"
            | "await"
            | "implements"
            | "interface"
            | "package"
            | "private"
            | "protected"
            | "public"
    )
}

// ============================================================
// Helper structs
// ============================================================

struct ObjectPatShorthandConverter;

impl VisitMut for ObjectPatShorthandConverter {
    fn visit_mut_object_pat(&mut self, obj: &mut ObjectPat) {
        obj.visit_mut_children_with(self);

        let new_props: Vec<ObjectPatProp> = obj
            .props
            .drain(..)
            .map(|prop| match prop {
                ObjectPatProp::KeyValue(kv) => {
                    let key_str = match &kv.key {
                        PropName::Ident(i) => Some(i.sym.clone()),
                        _ => None,
                    };
                    let alias = match kv.value.as_ref() {
                        Pat::Ident(bi) => Some(bi.id.sym.clone()),
                        Pat::Assign(ap) => match ap.left.as_ref() {
                            Pat::Ident(bi) => Some(bi.id.sym.clone()),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let (Some(k), Some(a)) = (key_str, alias) {
                        if k == a {
                            match *kv.value {
                                Pat::Ident(bi) => {
                                    return ObjectPatProp::Assign(AssignPatProp {
                                        span: bi.id.span,
                                        key: bi,
                                        value: None,
                                    });
                                }
                                Pat::Assign(ap) => {
                                    if let Pat::Ident(bi) = *ap.left {
                                        return ObjectPatProp::Assign(AssignPatProp {
                                            span: bi.id.span,
                                            key: bi,
                                            value: Some(ap.right),
                                        });
                                    }
                                    return ObjectPatProp::KeyValue(KeyValuePatProp {
                                        key: PropName::Ident(swc_core::ecma::ast::IdentName::new(
                                            k, DUMMY_SP,
                                        )),
                                        value: Box::new(Pat::Assign(ap)),
                                    });
                                }
                                other => {
                                    return ObjectPatProp::KeyValue(KeyValuePatProp {
                                        key: PropName::Ident(swc_core::ecma::ast::IdentName::new(
                                            k, DUMMY_SP,
                                        )),
                                        value: Box::new(other),
                                    });
                                }
                            }
                        }
                    }
                    ObjectPatProp::KeyValue(kv)
                }
                other => other,
            })
            .collect();
        obj.props = new_props;
    }
}

// ============================================================
// Member-init renames: var x = obj.prop → rename x to obj_prop
// ============================================================

fn member_init_rename_module(module: &mut Module) {
    let all_names = collect_names_in_module(&module.body);
    let renames = collect_member_init_renames_from_module(&module.body, &all_names);
    if renames.is_empty() {
        return;
    }
    rename_bindings_in_module(module, &renames);
}

fn member_init_rename_function(func: &mut Function) {
    let Some(body) = &mut func.body else { return };
    let mut all_names = collect_names_in_stmts(&body.stmts);
    for p in &func.params {
        collect_names_in_pat(&p.pat, &mut all_names);
    }
    let renames = collect_member_init_renames_from_stmts(&body.stmts, &all_names);
    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut body.stmts, &renames);
}

fn member_init_rename_arrow(arrow: &mut ArrowExpr) {
    let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_mut() else {
        return;
    };
    let mut all_names = collect_names_in_stmts(&block.stmts);
    for p in &arrow.params {
        collect_names_in_pat(p, &mut all_names);
    }
    let all_names = all_names;
    let renames = collect_member_init_renames_from_stmts(&block.stmts, &all_names);
    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut block.stmts, &renames);
}

fn collect_member_init_renames_from_module(
    body: &[ModuleItem],
    all_names: &HashSet<String>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();
    for item in body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        collect_member_init_var_renames(var, &mut renames, &mut used_names);
    }
    renames
}

fn collect_member_init_renames_from_stmts(
    stmts: &[Stmt],
    all_names: &HashSet<String>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        collect_member_init_var_renames(var, &mut renames, &mut used_names);
    }
    renames
}

fn collect_member_init_var_renames(
    var: &VarDecl,
    renames: &mut Vec<BindingRename>,
    used_names: &mut HashSet<String>,
) {
    for decl in &var.decls {
        let Pat::Ident(bi) = &decl.name else { continue };
        let Some(init) = &decl.init else { continue };
        let old_name = bi.id.sym.to_string();

        // Only rename short (minified) names
        if old_name.chars().count() > REACT_MINIFIED_THRESHOLD {
            continue;
        }

        // Match: var x = obj.prop
        let Expr::Member(member) = init.as_ref() else {
            continue;
        };
        let MemberProp::Ident(prop) = &member.prop else {
            continue;
        };
        let prop_name = prop.sym.to_string();

        // Build new name: obj_prop
        let new_name = if let Expr::Ident(obj) = member.obj.as_ref() {
            let obj_name = obj.sym.to_string();
            // Skip if both obj and prop are short — the combined name wouldn't help
            if obj_name.chars().count() <= REACT_MINIFIED_THRESHOLD
                && prop_name.chars().count() <= REACT_MINIFIED_THRESHOLD
            {
                continue;
            }
            format!("{}_{}", obj_name, prop_name)
        } else {
            // Non-ident obj (e.g. call().prop) — skip if prop is too short
            if prop_name.chars().count() <= REACT_MINIFIED_THRESHOLD {
                continue;
            }
            prop_name.clone()
        };

        let new_name = find_non_conflicting_name(&new_name, used_names);
        if new_name == old_name {
            continue;
        }
        used_names.insert(new_name.clone());
        renames.push(BindingRename {
            old: (bi.id.sym.clone(), bi.id.ctxt),
            new: new_name.as_str().into(),
        });
    }
}

// ============================================================
// Symbol.for("key") renames: var x = Symbol.for("react.element") → symbol_react_element
// ============================================================

fn symbol_for_rename_module(module: &mut Module) {
    let all_names = collect_names_in_module(&module.body);
    let renames = collect_symbol_for_renames_from_module(&module.body, &all_names);
    if renames.is_empty() {
        return;
    }
    rename_bindings_in_module(module, &renames);
}

fn symbol_for_rename_function(func: &mut Function) {
    let Some(body) = &mut func.body else { return };
    let mut all_names = collect_names_in_stmts(&body.stmts);
    for p in &func.params {
        collect_names_in_pat(&p.pat, &mut all_names);
    }
    let renames = collect_symbol_for_renames_from_stmts(&body.stmts, &all_names);
    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut body.stmts, &renames);
}

fn symbol_for_rename_arrow(arrow: &mut ArrowExpr) {
    let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_mut() else {
        return;
    };
    let mut all_names = collect_names_in_stmts(&block.stmts);
    for p in &arrow.params {
        collect_names_in_pat(p, &mut all_names);
    }
    let all_names = all_names;
    let renames = collect_symbol_for_renames_from_stmts(&block.stmts, &all_names);
    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut block.stmts, &renames);
}

fn collect_symbol_for_renames_from_module(
    body: &[ModuleItem],
    all_names: &HashSet<String>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();
    for item in body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                collect_symbol_for_var_renames(var, &mut renames, &mut used_names);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ed)) => {
                if let Decl::Var(var) = &ed.decl {
                    collect_symbol_for_var_renames(var, &mut renames, &mut used_names);
                }
            }
            _ => {}
        }
    }
    renames
}

fn collect_symbol_for_renames_from_stmts(
    stmts: &[Stmt],
    all_names: &HashSet<String>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        collect_symbol_for_var_renames(var, &mut renames, &mut used_names);
    }
    renames
}

fn collect_symbol_for_var_renames(
    var: &VarDecl,
    renames: &mut Vec<BindingRename>,
    used_names: &mut HashSet<String>,
) {
    for decl in &var.decls {
        let Pat::Ident(bi) = &decl.name else { continue };
        let Some(init) = &decl.init else { continue };
        let old_name = bi.id.sym.to_string();

        // Only rename short (minified) names
        if old_name.chars().count() > REACT_MINIFIED_THRESHOLD {
            continue;
        }

        // Match: Symbol.for("string")
        let Some(key) = extract_symbol_for_key(init) else {
            continue;
        };

        // Build new name: SYMBOL_REACT_ELEMENT from "react.element"
        // SYMBOL_ prefix hints this is a Symbol.for value, not a string constant
        let new_name = format!("SYMBOL_{}", symbol_key_to_const_name(&key));

        // Skip if the derived name is too short to be helpful
        if new_name.chars().count() <= old_name.chars().count() {
            continue;
        }

        let new_name = find_non_conflicting_name(&new_name, used_names);
        if new_name == old_name {
            continue;
        }
        used_names.insert(new_name.clone());
        renames.push(BindingRename {
            old: (bi.id.sym.clone(), bi.id.ctxt),
            new: new_name.as_str().into(),
        });
    }
}

/// Extract the string key from `Symbol.for("key")`.
fn extract_symbol_for_key(expr: &Expr) -> Option<String> {
    let Expr::Call(CallExpr { callee, args, .. }) = expr else {
        return None;
    };
    let Callee::Expr(callee_expr) = callee else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = callee_expr.as_ref() else {
        return None;
    };
    let Expr::Ident(obj_id) = obj.as_ref() else {
        return None;
    };
    if obj_id.sym.as_ref() != "Symbol" {
        return None;
    }
    let MemberProp::Ident(prop_id) = prop else {
        return None;
    };
    if prop_id.sym.as_ref() != "for" {
        return None;
    }
    if args.len() != 1 {
        return None;
    }
    let Expr::Lit(Lit::Str(s)) = args[0].expr.as_ref() else {
        return None;
    };
    s.value.as_str().map(|s| s.to_string())
}

// ============================================================
// Name collection helpers
// ============================================================

fn collect_names_in_module(body: &[ModuleItem]) -> HashSet<String> {
    let mut collector = NameCollector::default();
    body.visit_with(&mut collector);
    collector.names
}

fn collect_names_in_stmts(stmts: &[Stmt]) -> HashSet<String> {
    let mut collector = NameCollector::default();
    stmts.visit_with(&mut collector);
    collector.names
}

fn collect_names_in_expr(expr: &Expr, names: &mut HashSet<String>) {
    let mut collector = NameCollector::default();
    expr.visit_with(&mut collector);
    names.extend(collector.names);
}

fn collect_names_in_pat(pat: &Pat, names: &mut HashSet<String>) {
    let mut collector = NameCollector::default();
    pat.visit_with(&mut collector);
    names.extend(collector.names);
}

#[derive(Default)]
struct NameCollector {
    names: HashSet<String>,
}

impl Visit for NameCollector {
    fn visit_ident(&mut self, id: &Ident) {
        self.names.insert(id.sym.to_string());
    }
}

// ============================================================
// String helpers
// ============================================================

fn pascal_case_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

/// Sanitize a non-identifier string into a valid JS identifier.
/// Replaces hyphens, dots, spaces with underscores. Strips other invalid chars.
/// Returns empty string if nothing usable remains.
fn sanitize_to_ident(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c == '-' || c == '.' || c == ' ' {
                '_'
            } else {
                c
            }
        })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
        .collect();
    // Ensure it starts with a valid identifier character
    if sanitized.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{}", sanitized)
    } else {
        sanitized
    }
}

/// Check if a string has valid JS identifier syntax (letters, digits, _, $).
/// Does NOT reject reserved keywords — `find_non_conflicting_name` handles those
/// with a `_` prefix. This only rejects strings that can never be identifiers
/// (e.g. "aria-current", "data.key", "123abc").
fn is_valid_js_ident(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' && first != '$' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Convert a Symbol.for key like "react.element" to UPPER_SNAKE_CASE: "REACT_ELEMENT".
/// Handles dots, hyphens, and camelCase boundaries as separators.
fn symbol_key_to_const_name(key: &str) -> String {
    let mut result = String::new();
    let mut prev_was_sep = true; // treat start as after separator
    for ch in key.chars() {
        if ch == '.' || ch == '-' || ch == '_' || ch == ' ' {
            if !result.is_empty() {
                result.push('_');
            }
            prev_was_sep = true;
        } else if ch.is_uppercase() && !prev_was_sep && !result.is_empty() {
            // camelCase boundary: "forwardRef" → "FORWARD_REF"
            result.push('_');
            result.push(ch.to_ascii_uppercase());
            prev_was_sep = false;
        } else {
            result.push(ch.to_ascii_uppercase());
            prev_was_sep = false;
        }
    }
    result
}

// ============================================================
// Value-position renames
//
// A short binding `x` (≤2 chars) used *only* as the value of object-literal
// KeyValue properties with a valid-identifier key, where every such key
// agrees on the same target name, is renamed to that name.
//
//   (e, t) => ({ ...e, error: t })      → (e, error) => ({ ...e, error })
//   import r from "m"; export default { Foo: r }
//                                        → import Foo from "m"; export default { Foo }
//
// Disqualified:
//   - Any non-value-position reference (member access, call arg, spread,
//     assignment target, export specifier, etc.)
//   - Multiple distinct target names (e.g. `{ array: e, bool: e }`)
//   - Computed/numeric/reserved-keyword keys
// ============================================================

fn value_position_rename_module(module: &mut Module) {
    let mut collector = BindingCollector::default();
    module.visit_with(&mut collector);
    if collector.short_bindings.is_empty() {
        return;
    }
    let exported_bindings = collect_exported_binding_ids(module);

    let mut classifier = ValuePositionClassifier::new(collector.short_bindings);
    module.visit_with(&mut classifier);

    // Group candidates by target name. If two bindings map to the same
    // target (e.g. five React type constants all assigned to `$$typeof:`),
    // the key isn't discriminative — drop the whole group.
    let mut by_target: HashMap<String, Vec<BindingId>> = HashMap::new();
    for (bid, state) in classifier.states {
        let Some(target) = state.single_target() else {
            continue;
        };
        if target.as_str() == bid.0.as_ref() {
            continue;
        }
        by_target.entry(target).or_default().push(bid);
    }

    let top_level_names = collect_module_names(module);

    // Collect eligible candidates, sorted by target name for deterministic
    // output (HashMap iteration order is random).
    let mut candidates: Vec<(String, BindingId)> = by_target
        .into_iter()
        .filter_map(|(target, bids)| {
            if bids.len() > 1 {
                return None;
            }
            let bid = bids.into_iter().next().unwrap();
            if exported_bindings.contains(&bid) {
                return None;
            }
            Some((target, bid))
        })
        .collect();
    candidates.sort_by(|(a, _), (b, _)| a.cmp(b));

    // Two-pass assignment: first reserve direct (unsuffixed) target names so
    // a later suffix fallback never steals another binding's natural target.
    let mut renames: Vec<BindingRename> = Vec::new();
    let mut committed_names: HashSet<String> = HashSet::new();
    let mut needs_suffix: Vec<(String, BindingId)> = Vec::new();

    for (target, bid) in candidates {
        let atom: Atom = target.as_str().into();
        if !top_level_names.contains(&atom)
            && !rename_causes_shadowing(module, &bid, &atom)
        {
            committed_names.insert(target.clone());
            renames.push(BindingRename {
                old: bid,
                new: atom,
            });
        } else {
            needs_suffix.push((target, bid));
        }
    }

    for (target, bid) in needs_suffix {
        let final_name = (1..=10)
            .map(|i| format!("{target}_{i}"))
            .find(|candidate| {
                let atom: Atom = candidate.as_str().into();
                !committed_names.contains(candidate.as_str())
                    && !top_level_names.contains(&atom)
                    && !rename_causes_shadowing(module, &bid, &atom)
            });

        if let Some(name) = final_name {
            committed_names.insert(name.clone());
            renames.push(BindingRename {
                old: bid,
                new: name.as_str().into(),
            });
        }
    }

    if renames.is_empty() {
        return;
    }
    rename_bindings_in_module(module, &renames);
    // Collapse `{ Foo: Foo }` created by the rename back to `{ Foo }`.
    module.visit_mut_with(&mut ObjShorthand);
}

#[derive(Default)]
struct BindingCollector {
    short_bindings: HashMap<BindingId, ()>,
}

impl BindingCollector {
    fn record(&mut self, id: &Ident) {
        if id.sym.chars().count() <= REACT_MINIFIED_THRESHOLD {
            self.short_bindings.insert((id.sym.clone(), id.ctxt), ());
        }
    }
}

fn collect_exported_binding_ids(module: &Module) -> HashSet<BindingId> {
    let mut ids = HashSet::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(module_decl) = item else {
            continue;
        };
        match module_decl {
            ModuleDecl::ExportDecl(export) => {
                collect_decl_binding_ids(&export.decl, &mut ids);
            }
            ModuleDecl::ExportNamed(export) => {
                for specifier in &export.specifiers {
                    let ExportSpecifier::Named(named) = specifier else {
                        continue;
                    };
                    if let ModuleExportName::Ident(local) = &named.orig {
                        ids.insert((local.sym.clone(), local.ctxt));
                    }
                }
            }
            _ => {}
        }
    }
    ids
}

fn collect_decl_binding_ids(decl: &Decl, ids: &mut HashSet<BindingId>) {
    match decl {
        Decl::Var(var) => {
            for declarator in &var.decls {
                collect_pat_binding_ids(&declarator.name, ids);
            }
        }
        Decl::Fn(function) => {
            ids.insert((function.ident.sym.clone(), function.ident.ctxt));
        }
        Decl::Class(class) => {
            ids.insert((class.ident.sym.clone(), class.ident.ctxt));
        }
        _ => {}
    }
}

fn collect_pat_binding_ids(pat: &Pat, ids: &mut HashSet<BindingId>) {
    match pat {
        Pat::Ident(binding) => {
            ids.insert((binding.id.sym.clone(), binding.id.ctxt));
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_binding_ids(elem, ids);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat_binding_ids(&kv.value, ids),
                    ObjectPatProp::Assign(assign) => {
                        ids.insert((assign.key.sym.clone(), assign.key.ctxt));
                    }
                    ObjectPatProp::Rest(rest) => collect_pat_binding_ids(&rest.arg, ids),
                }
            }
        }
        Pat::Assign(assign) => collect_pat_binding_ids(&assign.left, ids),
        Pat::Rest(rest) => collect_pat_binding_ids(&rest.arg, ids),
        _ => {}
    }
}

impl Visit for BindingCollector {
    fn visit_pat(&mut self, pat: &Pat) {
        if let Pat::Ident(bi) = pat {
            self.record(&bi.id);
        }
        pat.visit_children_with(self);
    }

    fn visit_object_pat_prop(&mut self, prop: &ObjectPatProp) {
        if let ObjectPatProp::Assign(a) = prop {
            self.record(&a.key.id);
        }
        prop.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.record(&decl.ident);
        decl.function.visit_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        self.record(&decl.ident);
        decl.class.visit_with(self);
    }

    fn visit_fn_expr(&mut self, fn_expr: &FnExpr) {
        if let Some(ident) = &fn_expr.ident {
            self.record(ident);
        }
        fn_expr.function.visit_with(self);
    }

    fn visit_class_expr(&mut self, ce: &ClassExpr) {
        if let Some(ident) = &ce.ident {
            self.record(ident);
        }
        ce.class.visit_with(self);
    }

    fn visit_import_decl(&mut self, decl: &ImportDecl) {
        for spec in &decl.specifiers {
            match spec {
                ImportSpecifier::Default(d) => self.record(&d.local),
                ImportSpecifier::Named(n) => self.record(&n.local),
                ImportSpecifier::Namespace(ns) => self.record(&ns.local),
            }
        }
    }

    fn visit_prop_name(&mut self, name: &PropName) {
        if let PropName::Computed(c) = name {
            c.expr.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.expr.visit_with(self);
        }
    }
}

#[derive(Default)]
struct ClassificationState {
    value_targets: HashMap<String, usize>,
    other_uses: usize,
}

impl ClassificationState {
    fn single_target(&self) -> Option<String> {
        if self.other_uses > 0 {
            return None;
        }
        if self.value_targets.len() != 1 {
            return None;
        }
        self.value_targets.keys().next().cloned()
    }
}

struct ValuePositionClassifier {
    states: HashMap<BindingId, ClassificationState>,
}

impl ValuePositionClassifier {
    fn new(bindings: HashMap<BindingId, ()>) -> Self {
        let states = bindings
            .into_keys()
            .map(|k| (k, ClassificationState::default()))
            .collect();
        Self { states }
    }

    fn record_value_use(&mut self, bid: &BindingId, target: String) {
        if let Some(state) = self.states.get_mut(bid) {
            *state.value_targets.entry(target).or_default() += 1;
        }
    }

    fn record_other_use(&mut self, bid: &BindingId) {
        if let Some(state) = self.states.get_mut(bid) {
            state.other_uses += 1;
        }
    }
}

impl Visit for ValuePositionClassifier {
    fn visit_prop(&mut self, prop: &Prop) {
        // Handle the `{ Key: x }` value position specially so we don't
        // double-count the value Ident as a generic "other use".
        if let Prop::KeyValue(kv) = prop {
            if let PropName::Computed(c) = &kv.key {
                c.expr.visit_with(self);
            }
            if let Expr::Ident(id) = kv.value.as_ref() {
                let bid = (id.sym.clone(), id.ctxt);
                if self.states.contains_key(&bid) {
                    match key_as_ident_target(&kv.key) {
                        Some(name) => self.record_value_use(&bid, name),
                        None => self.record_other_use(&bid),
                    }
                    return;
                }
            }
            kv.value.visit_with(self);
            return;
        }
        prop.visit_children_with(self);
    }

    fn visit_ident(&mut self, id: &Ident) {
        let bid = (id.sym.clone(), id.ctxt);
        self.record_other_use(&bid);
    }

    // Patterns contain binding sites (declarations), not uses — walk manually
    // so we only descend into parts that can contain expressions (default
    // initializers, computed keys).
    fn visit_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(_) => {}
            Pat::Array(a) => {
                for elem in a.elems.iter().flatten() {
                    self.visit_pat(elem);
                }
            }
            Pat::Object(o) => {
                for prop in &o.props {
                    match prop {
                        ObjectPatProp::KeyValue(kv) => {
                            if let PropName::Computed(c) = &kv.key {
                                c.expr.visit_with(self);
                            }
                            self.visit_pat(&kv.value);
                        }
                        ObjectPatProp::Assign(ap) => {
                            if let Some(v) = &ap.value {
                                v.visit_with(self);
                            }
                        }
                        ObjectPatProp::Rest(rp) => self.visit_pat(&rp.arg),
                    }
                }
            }
            Pat::Assign(a) => {
                self.visit_pat(&a.left);
                a.right.visit_with(self);
            }
            Pat::Rest(r) => self.visit_pat(&r.arg),
            Pat::Expr(e) => e.visit_with(self),
            Pat::Invalid(_) => {}
        }
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        decl.function.visit_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        decl.class.visit_with(self);
    }

    fn visit_fn_expr(&mut self, fn_expr: &FnExpr) {
        fn_expr.function.visit_with(self);
    }

    fn visit_class_expr(&mut self, ce: &ClassExpr) {
        ce.class.visit_with(self);
    }

    fn visit_import_decl(&mut self, _: &ImportDecl) {
        // Import specifier locals are bindings, not uses.
    }

    fn visit_prop_name(&mut self, name: &PropName) {
        if let PropName::Computed(c) = name {
            c.expr.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.expr.visit_with(self);
        }
    }
}

fn key_as_ident_target(key: &PropName) -> Option<String> {
    let raw = match key {
        PropName::Ident(i) => i.sym.to_string(),
        PropName::Str(s) => s.value.as_str().map(|s| s.to_string())?,
        _ => return None,
    };
    if raw.is_empty() || !is_valid_js_ident(&raw) || is_reserved_keyword(&raw) {
        return None;
    }
    Some(raw)
}

// ============================================================
// JSX component alias renames
//
// SmartInline can leave a post-JSX alias when a lowercase value must be used
// as a component tag:
//
//   const Tm = sideCar;
//   return <Tm />;
//
// If the alias is a const binding and it is only used as a JSX tag, rename the
// alias from the source value instead of keeping the minified name:
//
//   const SideCar = sideCar;
//   return <SideCar />;
// ============================================================

fn jsx_component_alias_rename_module(module: &mut Module) {
    let mut collector = JsxComponentAliasCollector::default();
    module.visit_with(&mut collector);
    if collector.aliases.is_empty() {
        return;
    }

    let mut classifier = JsxComponentAliasClassifier::new(collector.aliases);
    module.visit_with(&mut classifier);

    let mut renames = Vec::new();
    for (bid, state) in classifier.states {
        if state.other_uses > 0 || state.jsx_uses == 0 {
            continue;
        }
        if collector.all_binding_names.contains(state.target.as_str()) {
            continue;
        }
        renames.push(BindingRename {
            old: bid,
            new: state.target.into(),
        });
    }

    rename_bindings_in_module(module, &renames);
}

#[derive(Default)]
struct JsxComponentAliasCollector {
    aliases: HashMap<BindingId, String>,
    all_binding_names: HashSet<String>,
}

impl JsxComponentAliasCollector {
    fn record_binding_name(&mut self, id: &Ident) {
        self.all_binding_names.insert(id.sym.to_string());
    }

    fn collect_pat_names(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(binding) => self.record_binding_name(&binding.id),
            Pat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.collect_pat_names(elem);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        ObjectPatProp::KeyValue(kv) => self.collect_pat_names(&kv.value),
                        ObjectPatProp::Assign(assign) => self.record_binding_name(&assign.key),
                        ObjectPatProp::Rest(rest) => self.collect_pat_names(&rest.arg),
                    }
                }
            }
            Pat::Assign(assign) => self.collect_pat_names(&assign.left),
            Pat::Rest(rest) => self.collect_pat_names(&rest.arg),
            _ => {}
        }
    }
}

impl Visit for JsxComponentAliasCollector {
    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.record_binding_name(&decl.ident);
        decl.function.visit_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        self.record_binding_name(&decl.ident);
        decl.class.visit_with(self);
    }

    fn visit_import_decl(&mut self, decl: &ImportDecl) {
        for spec in &decl.specifiers {
            match spec {
                ImportSpecifier::Default(default) => self.record_binding_name(&default.local),
                ImportSpecifier::Named(named) => self.record_binding_name(&named.local),
                ImportSpecifier::Namespace(namespace) => self.record_binding_name(&namespace.local),
            }
        }
    }

    fn visit_var_decl(&mut self, decl: &VarDecl) {
        for declarator in &decl.decls {
            self.collect_pat_names(&declarator.name);
            if decl.kind != VarDeclKind::Const {
                continue;
            }
            let Pat::Ident(binding) = &declarator.name else {
                continue;
            };
            if binding.id.sym.chars().count() > REACT_MINIFIED_THRESHOLD {
                continue;
            }
            let Some(Expr::Ident(source)) = declarator.init.as_deref() else {
                continue;
            };
            if !starts_with_lowercase(source.sym.as_ref()) {
                continue;
            }
            let target = pascalize(source.sym.as_ref());
            if target == binding.id.sym.as_ref() {
                continue;
            }
            self.aliases
                .insert((binding.id.sym.clone(), binding.id.ctxt), target);
        }

        decl.visit_children_with(self);
    }
}

struct JsxComponentAliasState {
    target: String,
    jsx_uses: usize,
    other_uses: usize,
}

struct JsxComponentAliasClassifier {
    states: HashMap<BindingId, JsxComponentAliasState>,
}

impl JsxComponentAliasClassifier {
    fn new(aliases: HashMap<BindingId, String>) -> Self {
        let states = aliases
            .into_iter()
            .map(|(bid, target)| {
                (
                    bid,
                    JsxComponentAliasState {
                        target,
                        jsx_uses: 0,
                        other_uses: 0,
                    },
                )
            })
            .collect();
        Self { states }
    }

    fn record_jsx_use(&mut self, ident: &Ident) {
        let bid = (ident.sym.clone(), ident.ctxt);
        if let Some(state) = self.states.get_mut(&bid) {
            state.jsx_uses += 1;
        }
    }

    fn record_other_use(&mut self, ident: &Ident) {
        let bid = (ident.sym.clone(), ident.ctxt);
        if let Some(state) = self.states.get_mut(&bid) {
            state.other_uses += 1;
        }
    }

    fn visit_binding_pat_defaults(&mut self, pat: &Pat) {
        match pat {
            Pat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.visit_binding_pat_defaults(elem);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        ObjectPatProp::KeyValue(kv) => self.visit_binding_pat_defaults(&kv.value),
                        ObjectPatProp::Assign(assign) => {
                            if let Some(default) = &assign.value {
                                default.visit_with(self);
                            }
                        }
                        ObjectPatProp::Rest(rest) => self.visit_binding_pat_defaults(&rest.arg),
                    }
                }
            }
            Pat::Assign(assign) => {
                self.visit_binding_pat_defaults(&assign.left);
                assign.right.visit_with(self);
            }
            Pat::Rest(rest) => self.visit_binding_pat_defaults(&rest.arg),
            Pat::Expr(expr) => expr.visit_with(self),
            Pat::Ident(_) | Pat::Invalid(_) => {}
        }
    }
}

impl Visit for JsxComponentAliasClassifier {
    fn visit_ident(&mut self, ident: &Ident) {
        self.record_other_use(ident);
    }

    fn visit_jsx_element_name(&mut self, name: &JSXElementName) {
        match name {
            JSXElementName::Ident(ident) => self.record_jsx_use(ident),
            JSXElementName::JSXMemberExpr(member) => self.visit_jsx_member_expr(member),
            JSXElementName::JSXNamespacedName(_) => {}
        }
    }

    fn visit_jsx_member_expr(&mut self, member: &JSXMemberExpr) {
        match &member.obj {
            JSXObject::Ident(ident) => self.record_other_use(ident),
            JSXObject::JSXMemberExpr(member) => self.visit_jsx_member_expr(member),
        }
    }

    fn visit_var_declarator(&mut self, declarator: &swc_core::ecma::ast::VarDeclarator) {
        self.visit_binding_pat_defaults(&declarator.name);
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_pat(&mut self, pat: &Pat) {
        self.visit_binding_pat_defaults(pat);
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        decl.function.visit_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        decl.class.visit_with(self);
    }

    fn visit_import_decl(&mut self, _: &ImportDecl) {}

    fn visit_prop_name(&mut self, name: &PropName) {
        if let PropName::Computed(computed) = name {
            computed.expr.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            computed.expr.visit_with(self);
        }
    }
}

fn pascalize(input: &str) -> String {
    let mut output = String::new();
    let mut capitalize = true;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            if capitalize {
                output.extend(ch.to_uppercase());
                capitalize = false;
            } else {
                output.push(ch);
            }
        } else {
            capitalize = true;
        }
    }
    if output.is_empty() {
        "Component".to_string()
    } else {
        output
    }
}

fn starts_with_lowercase(value: &str) -> bool {
    value
        .chars()
        .next()
        .map(|ch| ch.is_ascii_lowercase())
        .unwrap_or(false)
}
