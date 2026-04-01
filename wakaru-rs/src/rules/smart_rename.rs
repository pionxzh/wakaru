use std::collections::HashSet;

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrayPat, ArrowExpr, AssignPatProp, BlockStmtOrExpr, CallExpr, Callee, Decl, Expr, Function,
    Ident, KeyValuePatProp, MemberProp, Module, ModuleDecl, ModuleItem, ObjectPat, ObjectPatProp,
    Param, Pat, PropName, Stmt, VarDecl,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::rename_utils::{rename_bindings, rename_bindings_in_module, BindingId, BindingRename};

pub struct SmartRename;

impl VisitMut for SmartRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        react_rename_module(module);
        destructuring_rename_module(module);
        member_init_rename_module(module);
        module.visit_mut_children_with(self);
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        react_rename_function_body(func);
        destructuring_rename_function(func);
        member_init_rename_function(func);
        func.visit_mut_children_with(self);
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        destructuring_rename_arrow(arrow);
        member_init_rename_arrow(arrow);
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
        "useReducer" => args.len() >= 1 && args.len() <= 3,
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
    let all_names = match arrow.body.as_ref() {
        BlockStmtOrExpr::BlockStmt(b) => collect_names_in_stmts(&b.stmts),
        BlockStmtOrExpr::Expr(e) => {
            let mut names = HashSet::new();
            collect_names_in_expr(e, &mut names);
            names
        }
    };
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
        let Stmt::Decl(Decl::Var(var)) = stmt else { continue };
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
                let alias = match extract_binding_from_pat(&kv.value) {
                    Some(id) => id,
                    None => continue,
                };
                if alias.0.as_ref().chars().count() > REACT_MINIFIED_THRESHOLD {
                    continue;
                }
                if alias.0.as_ref() == key_str {
                    continue;
                }
                let new_name = find_non_conflicting_name(&key_str, used_names);
                used_names.insert(new_name.clone());
                renames.push(BindingRename {
                    old: alias,
                    new: new_name.as_str().into(),
                });
            }
            ObjectPatProp::Assign(_) | ObjectPatProp::Rest(_) => {}
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
    let all_names = collect_names_in_stmts(&block.stmts);
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
        let Stmt::Decl(Decl::Var(var)) = stmt else { continue };
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
        let Expr::Member(member) = init.as_ref() else { continue };
        let MemberProp::Ident(prop) = &member.prop else { continue };
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
