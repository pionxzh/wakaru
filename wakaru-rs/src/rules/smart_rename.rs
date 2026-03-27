use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowExpr, ArrayPat, AssignPatProp, BlockStmtOrExpr, CallExpr, Callee,
    Decl, Expr, Function, Ident, KeyValuePatProp, MemberProp,
    Module, ModuleDecl, ModuleItem, ObjectPat, ObjectPatProp, Param, Pat, PropName, Stmt,
    VarDecl,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub struct SmartRename;

impl VisitMut for SmartRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        // 1) React hook renames (rename return values of hook calls)
        react_rename_module(module);
        // 2) Destructuring shorthand renames
        destructuring_rename_module(module);
        // 3) Recurse into nested scopes (functions, etc.)
        module.visit_mut_children_with(self);
    }

    // Prevent double-processing nested scopes: visit_mut_module handles
    // the top level and recurse, but inner Function/ArrowExpr are handled
    // by visit_mut_children_with above, not by fresh calls to the outer logic.
    // So we override visit_mut_function and visit_mut_arrow_expr to
    // process their own scopes before recursing.
    fn visit_mut_function(&mut self, func: &mut Function) {
        react_rename_function_body(func);
        destructuring_rename_function(func);
        func.visit_mut_children_with(self);
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        destructuring_rename_arrow(arrow);
        arrow.visit_mut_children_with(self);
    }
}

// ============================================================
// React hook renames
// ============================================================

const REACT_MINIFIED_THRESHOLD: usize = 2;

fn react_rename_module(module: &mut Module) {
    let all_names = collect_names_in_module(&module.body);
    for item in &mut module.body {
        if let ModuleItem::Stmt(stmt) = item {
            react_rename_stmt_with_names(stmt, &all_names);
        }
    }
}

fn react_rename_function_body(func: &mut Function) {
    if let Some(body) = &mut func.body {
        let all_names = collect_names_in_stmts(&body.stmts);
        for stmt in &mut body.stmts {
            react_rename_stmt_with_names(stmt, &all_names);
        }
    }
}

fn react_rename_stmt_with_names(stmt: &mut Stmt, all_names: &HashSet<String>) {
    let Stmt::Decl(Decl::Var(var_decl)) = stmt else {
        return;
    };
    process_react_var_decl(var_decl, all_names);
}

fn process_react_var_decl(var_decl: &mut VarDecl, all_names: &HashSet<String>) {
    for decl in &mut var_decl.decls {
        match &mut decl.name {
            Pat::Ident(binding) => {
                // useRef / createContext: const x = useRef()/createContext()
                if let Some(init) = &decl.init {
                    if let Some(hook_name) = get_single_react_hook_call(init) {
                        let old = binding.id.sym.as_str().to_string();
                        if old.chars().count() <= REACT_MINIFIED_THRESHOLD {
                            let new_name = match hook_name.as_str() {
                                "useRef" => format!("{}Ref", old),
                                "createContext" => pascal_case_first(&old) + "Context",
                                _ => continue,
                            };
                            if !all_names.contains(&new_name) || new_name == old {
                                let old_atom: Atom = old.as_str().into();
                                let new_atom: Atom = new_name.as_str().into();
                                rename_ident_in_var_decl(var_decl, &old_atom, &new_atom);
                                // We modified var_decl — need to stop iterating (borrow check)
                                // rename_ident_in_var_decl handles the decl.name update too
                                return;
                            }
                        }
                    }
                }
            }
            Pat::Array(array_pat) => {
                // useState: const [state, setter] = useState(...)
                // useReducer: const [state, dispatch] = useReducer(...)
                if let Some(init) = &decl.init {
                    if let Some(hook_name) = get_single_react_hook_call(init) {
                        process_array_pat_react(array_pat, &hook_name, all_names);
                    }
                }
            }
            _ => {}
        }
    }
}

fn rename_ident_in_var_decl(var_decl: &mut VarDecl, old: &Atom, new: &Atom) {
    let mut renamer = IdentSymRenamer { old: old.clone(), new: new.clone() };
    var_decl.visit_mut_with(&mut renamer);
}

fn process_array_pat_react(array_pat: &mut ArrayPat, hook_name: &str, all_names: &HashSet<String>) {
    match hook_name {
        "useState" => {
            // [state, setter] — rename setter to setX
            // X = pascalCase(state) if state present, else pascalCase(setter)
            let state_name = get_array_elem_name(array_pat, 0);
            if let Some(setter_name) = get_array_elem_name_if_short(array_pat, 1) {
                let base = state_name.unwrap_or_else(|| setter_name.clone());
                let new_setter = format!("set{}", pascal_case_first(&base));
                if !all_names.contains(&new_setter) || new_setter == setter_name {
                    if let Some(Some(Pat::Ident(bi))) = array_pat.elems.get_mut(1) {
                        bi.id.sym = new_setter.as_str().into();
                    }
                }
            }
        }
        "useReducer" => {
            // [state, dispatch] — rename to [stateState, dispatchDispatch]
            if let Some(state_name) = get_array_elem_name_if_short(array_pat, 0) {
                let new_state = format!("{}State", state_name);
                if !all_names.contains(&new_state) || new_state == state_name {
                    if let Some(Some(Pat::Ident(bi))) = array_pat.elems.get_mut(0) {
                        bi.id.sym = new_state.as_str().into();
                    }
                }
            }
            if let Some(dispatch_name) = get_array_elem_name_if_short(array_pat, 1) {
                let new_dispatch = format!("{}Dispatch", dispatch_name);
                if !all_names.contains(&new_dispatch) || new_dispatch == dispatch_name {
                    if let Some(Some(Pat::Ident(bi))) = array_pat.elems.get_mut(1) {
                        bi.id.sym = new_dispatch.as_str().into();
                    }
                }
            }
        }
        _ => {}
    }
}

/// Returns the hook name if `expr` is a call to a known React hook (with ≤1 arg for useRef/createContext, ≤3 for useState/useReducer).
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

    if valid { Some(fn_name) } else { None }
}

fn get_array_elem_name(array_pat: &ArrayPat, idx: usize) -> Option<String> {
    if let Some(Some(Pat::Ident(bi))) = array_pat.elems.get(idx) {
        Some(bi.id.sym.to_string())
    } else {
        None
    }
}

fn get_array_elem_name_if_short(array_pat: &ArrayPat, idx: usize) -> Option<String> {
    let name = get_array_elem_name(array_pat, idx)?;
    if name.chars().count() <= REACT_MINIFIED_THRESHOLD {
        Some(name)
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
    apply_renames_to_module(module, &renames);
}

fn destructuring_rename_function(func: &mut Function) {
    let Some(body) = &func.body else { return };
    let mut all_names = collect_names_in_stmts(&body.stmts);
    for p in &func.params {
        collect_names_in_pat(&p.pat, &mut all_names);
    }
    let renames = collect_obj_pat_renames_from_params(&func.params, &all_names);
    if renames.is_empty() {
        return;
    }
    let mut renamer = MultiRenamer { renames };
    func.params.iter_mut().for_each(|p| p.visit_mut_with(&mut renamer));
    if let Some(body) = &mut func.body {
        body.visit_mut_with(&mut renamer);
    }
    // Convert { key: key } → { key } shorthand
    let mut shorthand = ObjectPatShorthandConverter;
    func.params.iter_mut().for_each(|p| p.visit_mut_with(&mut shorthand));
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
    let renames = collect_obj_pat_renames_from_pats(&arrow.params, &all_names);
    if renames.is_empty() {
        return;
    }
    let mut renamer = MultiRenamer { renames };
    arrow.params.iter_mut().for_each(|p| p.visit_mut_with(&mut renamer));
    arrow.body.visit_mut_with(&mut renamer);
    let mut shorthand = ObjectPatShorthandConverter;
    arrow.params.iter_mut().for_each(|p| p.visit_mut_with(&mut shorthand));
}

/// Returns renames from ObjectPat properties in var decls and function params at module level.
fn collect_obj_pat_renames_from_module(
    body: &[ModuleItem],
    all_names: &HashSet<String>,
) -> Vec<(Atom, Atom)> {
    let mut renames = Vec::new();
    let mut used_names: HashSet<String> = all_names.clone();

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
) -> Vec<(Atom, Atom)> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();
    for p in params {
        collect_obj_pat_renames_from_pat(&p.pat, &mut renames, &mut used_names);
    }
    renames
}

fn collect_obj_pat_renames_from_pats(
    params: &[Pat],
    all_names: &HashSet<String>,
) -> Vec<(Atom, Atom)> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();
    for p in params {
        collect_obj_pat_renames_from_pat(p, &mut renames, &mut used_names);
    }
    renames
}

fn collect_obj_pat_renames_from_pat(
    pat: &Pat,
    renames: &mut Vec<(Atom, Atom)>,
    used_names: &mut HashSet<String>,
) {
    let Pat::Object(obj_pat) = pat else { return };
    for prop in &obj_pat.props {
        match prop {
            ObjectPatProp::KeyValue(kv) => {
                // { key: alias } or { key: alias = default }
                let key_str = match &kv.key {
                    PropName::Ident(i) => i.sym.to_string(),
                    PropName::Str(s) => s.value.as_str().map(|s| s.to_string()).unwrap_or_default(),
                    _ => continue,
                };
                let alias = match extract_binding_from_pat(&kv.value) {
                    Some(id) => id,
                    None => continue,
                };
                if alias.chars().count() > REACT_MINIFIED_THRESHOLD {
                    continue;
                }
                // Check for same-name: { x: x } → shorthand { x } (no sym rename needed)
                if alias == key_str {
                    // Will be converted to shorthand by the pattern update, no sym rename
                    continue;
                }
                // Determine a non-conflicting new name
                let new_name = find_non_conflicting_name(&key_str, used_names);
                used_names.insert(new_name.clone());
                let old_atom: Atom = alias.as_str().into();
                let new_atom: Atom = new_name.as_str().into();
                renames.push((old_atom, new_atom));
            }
            ObjectPatProp::Assign(AssignPatProp { key, .. }) => {
                // { key } or { key = default } — already shorthand, no rename needed
                let _ = key;
            }
            ObjectPatProp::Rest(_) => {}
        }
    }
}

fn extract_binding_from_pat(pat: &Pat) -> Option<String> {
    match pat {
        Pat::Ident(bi) => Some(bi.id.sym.to_string()),
        Pat::Assign(assign_pat) => extract_binding_from_pat(&assign_pat.left),
        _ => None,
    }
}

fn find_non_conflicting_name(base: &str, used_names: &HashSet<String>) -> String {
    // Handle reserved keywords
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
        "break" | "case" | "catch" | "class" | "const" | "continue" | "debugger"
        | "default" | "delete" | "do" | "else" | "export" | "extends" | "false"
        | "finally" | "for" | "function" | "if" | "import" | "in" | "instanceof"
        | "let" | "new" | "null" | "return" | "static" | "super" | "switch"
        | "this" | "throw" | "true" | "try" | "typeof" | "var" | "void"
        | "while" | "with" | "yield" | "enum" | "await" | "implements"
        | "interface" | "package" | "private" | "protected" | "public"
    )
}

/// Apply the collected renames AND convert ObjectPat to shorthand wherever possible.
fn apply_renames_to_module(module: &mut Module, renames: &[(Atom, Atom)]) {
    // First rename all ident usages
    let mut renamer = MultiRenamer { renames: renames.to_vec() };
    module.visit_mut_with(&mut renamer);
    // Then convert ObjectPat KeyValue → shorthand where key == new_name
    let mut shorthand = ObjectPatShorthandConverter;
    module.visit_mut_with(&mut shorthand);
}

// ============================================================
// Helper structs
// ============================================================

struct IdentSymRenamer {
    old: Atom,
    new: Atom,
}

impl VisitMut for IdentSymRenamer {
    fn visit_mut_ident(&mut self, id: &mut Ident) {
        if id.sym == self.old {
            id.sym = self.new.clone();
        }
    }
    // Don't rename property identifier keys (dot notation): obj.foo
    fn visit_mut_prop_name(&mut self, _: &mut PropName) {}
    // For member props: only recurse into computed (bracket notation), skip dot notation
    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_mut_with(self);
        }
    }
}

struct MultiRenamer {
    renames: Vec<(Atom, Atom)>,
}

impl VisitMut for MultiRenamer {
    fn visit_mut_ident(&mut self, id: &mut Ident) {
        for (old, new) in &self.renames {
            if &id.sym == old {
                id.sym = new.clone();
                break;
            }
        }
    }
    fn visit_mut_prop_name(&mut self, _: &mut PropName) {}
    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_mut_with(self);
        }
    }
}

/// Convert `{ key: alias }` → `{ key }` (shorthand) where alias == key (post-rename).
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
                            // Convert to shorthand
                            match *kv.value {
                                Pat::Ident(bi) => {
                                    return ObjectPatProp::Assign(AssignPatProp {
                                        span: bi.id.span,
                                        key: bi,
                                        value: None,
                                    });
                                }
                                Pat::Assign(ap) => {
                                    // { key: alias = default } → { key = default }
                                    if let Pat::Ident(bi) = *ap.left {
                                        return ObjectPatProp::Assign(AssignPatProp {
                                            span: bi.id.span,
                                            key: bi,
                                            value: Some(ap.right),
                                        });
                                    }
                                    return ObjectPatProp::KeyValue(KeyValuePatProp {
                                        key: PropName::Ident(swc_core::ecma::ast::IdentName::new(k, DUMMY_SP)),
                                        value: Box::new(Pat::Assign(ap)),
                                    });
                                }
                                other => {
                                    return ObjectPatProp::KeyValue(KeyValuePatProp {
                                        key: PropName::Ident(swc_core::ecma::ast::IdentName::new(k, DUMMY_SP)),
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
