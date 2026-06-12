use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use swc_core::atoms::Atom;
use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, ArrowExpr, AssignPatProp, BlockStmtOrExpr, CallExpr, Callee, ClassDecl, ClassExpr,
    Decl, ExportSpecifier, Expr, FnDecl, FnExpr, Function, Ident, ImportDecl, ImportSpecifier,
    JSXAttr, JSXAttrName, JSXAttrOrSpread, JSXAttrValue, JSXElementName, JSXExpr, JSXExprContainer,
    JSXMemberExpr, JSXObject, KeyValuePatProp, Lit, MemberExpr, MemberProp, Module, ModuleDecl,
    ModuleExportName, ModuleItem, ObjectPat, ObjectPatProp, Param, Pat, Prop, PropName, Stmt,
    VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::js_names::{
    is_likely_generated_alias, is_reserved_binding_name, to_valid_identifier_name,
};

use super::decl_utils::collect_decl_binding_ids;
use super::expr_utils::is_unresolved_ident;
use super::extract_inlined_function::SharedExtractedFunctionNames;
use super::helper_matcher::static_member_prop_name;
use super::rename_utils::{
    collect_module_names, rename_bindings, rename_bindings_in_module, BindingId, BindingRename,
    RenameShadowIndex,
};
use super::ObjShorthand;

pub struct SmartRename {
    unresolved_mark: Mark,
    pending_value_position_names: HashMap<BindingId, String>,
    extracted_function_names: SharedExtractedFunctionNames,
}

impl SmartRename {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self {
            unresolved_mark,
            pending_value_position_names: HashMap::new(),
            extracted_function_names: Rc::new(RefCell::new(HashMap::new())),
        }
    }
}

impl VisitMut for SmartRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let previous_pending_names = std::mem::replace(
            &mut self.pending_value_position_names,
            collect_value_position_rename_map_module(module),
        );
        let mut cached_names = collect_names_in_module(&module.body);
        react_rename_module_with(
            module,
            &mut cached_names,
            &self.pending_value_position_names,
        );
        destructuring_rename_module_with(module, &mut cached_names);
        member_init_rename_module_with(module, &mut cached_names);
        symbol_for_rename_module_with(module, &mut cached_names, self.unresolved_mark);

        sentry_component_rename_module(module);
        react_function_shape_rename_module(module, &self.extracted_function_names.borrow());
        module.visit_mut_children_with(self);
        // Runs once at the module level; uses (sym, ctxt) matching so nested
        // bindings are classified correctly without per-scope recursion.
        value_position_rename_module(module);
        jsx_component_alias_rename_module(module);
        self.pending_value_position_names = previous_pending_names;
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        react_rename_function_body(func, &self.pending_value_position_names);
        destructuring_rename_function(func);
        member_init_rename_function(func);
        symbol_for_rename_function(func, self.unresolved_mark);
        func.visit_mut_children_with(self);
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        react_rename_arrow_body(arrow, &self.pending_value_position_names);
        destructuring_rename_arrow(arrow);
        member_init_rename_arrow(arrow);
        symbol_for_rename_arrow(arrow, self.unresolved_mark);
        arrow.visit_mut_children_with(self);
    }
}

/// Second pass of SmartRename that skips module-level non-JSX sub-rules
/// (react hooks, destructuring, member-init, Symbol.for) which were fully
/// handled by the first pass, but keeps the recursive descent for function-
/// level sub-rules that can benefit from intermediate pipeline rules
/// (e.g. UnIife2 exposing new React hook patterns).
pub struct SmartRenameSecondPass {
    unresolved_mark: Mark,
    pending_value_position_names: HashMap<BindingId, String>,
    extracted_function_names: SharedExtractedFunctionNames,
}

impl SmartRenameSecondPass {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self::new_with_extracted_function_names(unresolved_mark, Default::default())
    }

    pub fn new_with_extracted_function_names(
        unresolved_mark: Mark,
        extracted_function_names: SharedExtractedFunctionNames,
    ) -> Self {
        Self {
            unresolved_mark,
            pending_value_position_names: HashMap::new(),
            extracted_function_names,
        }
    }
}

impl VisitMut for SmartRenameSecondPass {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let previous_pending_names = std::mem::replace(
            &mut self.pending_value_position_names,
            collect_value_position_rename_map_module(module),
        );
        sentry_component_rename_module(module);
        react_function_shape_rename_module(module, &self.extracted_function_names.borrow());
        module.visit_mut_children_with(self);
        value_position_rename_module(module);
        jsx_component_alias_rename_module(module);
        self.pending_value_position_names = previous_pending_names;
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        react_rename_function_body(func, &self.pending_value_position_names);
        destructuring_rename_function(func);
        member_init_rename_function(func);
        symbol_for_rename_function(func, self.unresolved_mark);
        func.visit_mut_children_with(self);
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        react_rename_arrow_body(arrow, &self.pending_value_position_names);
        destructuring_rename_arrow(arrow);
        member_init_rename_arrow(arrow);
        symbol_for_rename_arrow(arrow, self.unresolved_mark);
        arrow.visit_mut_children_with(self);
    }
}

// ============================================================
// React hook renames
// ============================================================

const MAX_SYNTHETIC_NAME_ATTEMPTS: usize = 10_000;

fn react_rename_module_with(
    module: &mut Module,
    all_names: &mut HashSet<Atom>,
    pending_value_position_names: &HashMap<BindingId, String>,
) {
    let renames = collect_react_renames_from_module_items(
        &module.body,
        all_names,
        pending_value_position_names,
    );
    if renames.is_empty() {
        return;
    }
    for r in &renames {
        all_names.insert(r.new.clone());
    }
    rename_bindings_in_module(module, &renames);
}

fn react_rename_function_body(
    func: &mut Function,
    pending_value_position_names: &HashMap<BindingId, String>,
) {
    let Some(body) = &mut func.body else { return };
    if !has_react_candidates_in_stmts(&body.stmts) {
        return;
    }
    let all_names = collect_names_in_stmts(&body.stmts);
    let renames =
        collect_react_renames_from_stmts(&body.stmts, &all_names, pending_value_position_names);
    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut body.stmts, &renames);
}

fn react_rename_arrow_body(
    arrow: &mut ArrowExpr,
    pending_value_position_names: &HashMap<BindingId, String>,
) {
    let BlockStmtOrExpr::BlockStmt(body) = arrow.body.as_mut() else {
        return;
    };
    if !has_react_candidates_in_stmts(&body.stmts) {
        return;
    }
    let all_names = collect_names_in_stmts(&body.stmts);
    let renames =
        collect_react_renames_from_stmts(&body.stmts, &all_names, pending_value_position_names);
    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut body.stmts, &renames);
}

fn has_react_candidates_in_stmts(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| {
        let Stmt::Decl(Decl::Var(var_decl)) = stmt else {
            return false;
        };
        var_decl.decls.iter().any(|decl| {
            let Some(init) = &decl.init else { return false };
            let Some(hook_name) = get_single_react_hook_call(init) else {
                return false;
            };
            match &decl.name {
                Pat::Ident(bi) => is_likely_generated_alias(&bi.id.sym),
                Pat::Array(arr) => arr.elems.iter().enumerate().any(|(idx, elem)| {
                    let Some(Pat::Ident(bi)) = elem else {
                        return false;
                    };
                    is_likely_generated_alias(&bi.id.sym)
                        || (hook_name == "useState"
                            && idx == 1
                            && is_likely_generated_react_setter_alias(&bi.id.sym))
                }),
                _ => false,
            }
        })
    })
}

fn collect_react_renames_from_module_items(
    body: &[ModuleItem],
    all_names: &HashSet<Atom>,
    pending_value_position_names: &HashMap<BindingId, String>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();

    for item in body {
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item {
            collect_react_var_decl_renames(
                var_decl,
                &mut renames,
                &mut used_names,
                pending_value_position_names,
            );
        }
    }

    renames
}

fn collect_react_renames_from_stmts(
    stmts: &[Stmt],
    all_names: &HashSet<Atom>,
    pending_value_position_names: &HashMap<BindingId, String>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();

    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var_decl)) = stmt else {
            continue;
        };
        collect_react_var_decl_renames(
            var_decl,
            &mut renames,
            &mut used_names,
            pending_value_position_names,
        );
    }

    renames
}

fn collect_react_var_decl_renames(
    var_decl: &VarDecl,
    renames: &mut Vec<BindingRename>,
    used_names: &mut HashSet<Atom>,
    pending_value_position_names: &HashMap<BindingId, String>,
) {
    for decl in &var_decl.decls {
        match &decl.name {
            Pat::Ident(binding) => {
                if let Some(init) = &decl.init {
                    if let Some(hook_name) = get_single_react_hook_call(init) {
                        let old_name = binding.id.sym.to_string();
                        if !is_likely_generated_alias(&binding.id.sym) {
                            continue;
                        }

                        let new_name = match hook_name.as_str() {
                            "useRef" => format!("{}Ref", old_name),
                            "createContext" => pascal_case_first(&old_name) + "Context",
                            _ => continue,
                        };

                        let new_atom = Atom::from(new_name.as_str());
                        if !used_names.contains(&new_atom) || new_name == old_name {
                            used_names.insert(new_atom);
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
                        collect_array_pat_react_renames(
                            array_pat,
                            init,
                            &hook_name,
                            renames,
                            used_names,
                            pending_value_position_names,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn collect_array_pat_react_renames(
    array_pat: &ArrayPat,
    init: &Expr,
    hook_name: &str,
    renames: &mut Vec<BindingRename>,
    used_names: &mut HashSet<Atom>,
    pending_value_position_names: &HashMap<BindingId, String>,
) {
    match hook_name {
        "useState" => {
            let state_name = get_array_elem_binding(array_pat, 0).map(|(name, id)| {
                pending_value_position_names
                    .get(&id)
                    .cloned()
                    .unwrap_or(name)
            });
            if let Some((setter_name, setter_id, is_setter_alias)) =
                get_use_state_setter_candidate(array_pat, 1)
            {
                let new_setter = if let Some(base) = state_name {
                    Some(format!(
                        "set{}",
                        pascal_case_first(&react_state_setter_base_name(&base))
                    ))
                } else if is_setter_alias {
                    None
                } else {
                    Some(format!("set{}", pascal_case_first(&setter_name)))
                };

                if let Some(new_setter) = new_setter {
                    if new_setter != setter_name {
                        let new_setter = find_non_conflicting_name(&new_setter, used_names);
                        used_names.insert(Atom::from(new_setter.as_str()));
                        renames.push(BindingRename {
                            old: setter_id,
                            new: new_setter.as_str().into(),
                        });
                    }
                }
            }
        }
        "useReducer" => {
            if let Some((state_name, state_id)) = get_array_elem_if_short(array_pat, 0) {
                let new_state = format!("{}State", state_name);
                let state_atom = Atom::from(new_state.as_str());
                if !used_names.contains(&state_atom) || new_state == state_name {
                    used_names.insert(state_atom);
                    renames.push(BindingRename {
                        old: state_id,
                        new: new_state.as_str().into(),
                    });
                }
            }
            if let Some((dispatch_name, dispatch_id)) = get_array_elem_if_short(array_pat, 1) {
                let new_dispatch = format!("{}Dispatch", dispatch_name);
                let dispatch_atom = Atom::from(new_dispatch.as_str());
                if !used_names.contains(&dispatch_atom) || new_dispatch == dispatch_name {
                    used_names.insert(dispatch_atom);
                    renames.push(BindingRename {
                        old: dispatch_id,
                        new: new_dispatch.as_str().into(),
                    });
                }
            }
        }
        "useTransition" => {
            rename_array_elem_if_short(array_pat, 0, "isPending", renames, used_names);
            rename_array_elem_if_short(array_pat, 1, "startTransition", renames, used_names);
        }
        "useOptimistic" => {
            let Some(base) = optimistic_state_base_name(init, array_pat) else {
                return;
            };
            let optimistic_name = format!("optimistic{}", pascal_case_first(&base));
            rename_array_elem_if_short(array_pat, 0, &optimistic_name, renames, used_names);
            let setter_name = format!("set{}", pascal_case_first(&optimistic_name));
            rename_array_elem_if_short(array_pat, 1, &setter_name, renames, used_names);
        }
        _ => {}
    }
}

fn get_use_state_setter_candidate(
    array_pat: &ArrayPat,
    idx: usize,
) -> Option<(String, BindingId, bool)> {
    let Some(Some(Pat::Ident(bi))) = array_pat.elems.get(idx) else {
        return None;
    };
    let name = bi.id.sym.to_string();
    if is_likely_generated_alias(&bi.id.sym) {
        return Some((name, (bi.id.sym.clone(), bi.id.ctxt), false));
    }
    if is_likely_generated_react_setter_alias(&name) {
        return Some((name, (bi.id.sym.clone(), bi.id.ctxt), true));
    }
    None
}

fn is_likely_generated_react_setter_alias(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("set") else {
        return false;
    };
    !rest.is_empty() && is_likely_generated_alias(rest)
}

fn react_state_setter_base_name(name: &str) -> String {
    let Some((base, suffix)) = name.rsplit_once('_') else {
        return name.to_string();
    };
    if base.is_empty() || suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return name.to_string();
    }
    base.to_string()
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
        "useTransition" => args.is_empty(),
        "useOptimistic" => !args.is_empty() && args.len() <= 2,
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

fn get_array_elem_binding(array_pat: &ArrayPat, idx: usize) -> Option<(String, BindingId)> {
    let Some(Some(Pat::Ident(bi))) = array_pat.elems.get(idx) else {
        return None;
    };
    Some((bi.id.sym.to_string(), (bi.id.sym.clone(), bi.id.ctxt)))
}

fn get_array_elem_if_short(array_pat: &ArrayPat, idx: usize) -> Option<(String, BindingId)> {
    let Some(Some(Pat::Ident(bi))) = array_pat.elems.get(idx) else {
        return None;
    };
    let name = bi.id.sym.to_string();
    if is_likely_generated_alias(&bi.id.sym) {
        Some((name, (bi.id.sym.clone(), bi.id.ctxt)))
    } else {
        None
    }
}

fn rename_array_elem_if_short(
    array_pat: &ArrayPat,
    idx: usize,
    new_name: &str,
    renames: &mut Vec<BindingRename>,
    used_names: &mut HashSet<Atom>,
) {
    let Some((old_name, old_id)) = get_array_elem_if_short(array_pat, idx) else {
        return;
    };
    let new_name = find_non_conflicting_name(new_name, used_names);
    if new_name == old_name {
        return;
    }
    used_names.insert(Atom::from(new_name.as_str()));
    renames.push(BindingRename {
        old: old_id,
        new: new_name.as_str().into(),
    });
}

fn optimistic_state_base_name(init: &Expr, array_pat: &ArrayPat) -> Option<String> {
    let Expr::Call(call) = init else {
        return None;
    };
    if let Some(first_arg) = call.args.first() {
        if let Some(name) = optimistic_source_name(first_arg.expr.as_ref()) {
            return Some(strip_current_prefix(&name));
        }
    }

    let first_name = get_array_elem_name(array_pat, 0)?;
    if is_likely_generated_alias(first_name.as_str()) {
        return None;
    }
    Some(strip_optimistic_prefix(&first_name))
}

fn optimistic_source_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(id) => Some(id.sym.to_string()),
        Expr::Member(member) => match &member.prop {
            MemberProp::Ident(prop) => Some(prop.sym.to_string()),
            MemberProp::Computed(computed) => {
                let Expr::Lit(Lit::Str(value)) = computed.expr.as_ref() else {
                    return None;
                };
                value.value.as_str().map(|s| s.to_string())
            }
            _ => None,
        },
        _ => None,
    }
}

fn strip_current_prefix(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("current") {
        if rest
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
        {
            return lower_first(rest);
        }
    }
    name.to_string()
}

fn strip_optimistic_prefix(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("optimistic") {
        if rest
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
        {
            return lower_first(rest);
        }
    }
    name.to_string()
}

fn lower_first(input: &str) -> String {
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut result = String::new();
    result.extend(first.to_lowercase());
    result.extend(chars);
    result
}

// ============================================================
// Destructuring shorthand renames
// ============================================================

fn has_short_obj_pat_alias(pat: &Pat) -> bool {
    let Pat::Object(obj_pat) = pat else {
        return false;
    };
    obj_pat.props.iter().any(|prop| match prop {
        ObjectPatProp::KeyValue(kv) => extract_binding_from_pat(&kv.value)
            .is_some_and(|(sym, _)| is_likely_generated_alias(&sym)),
        ObjectPatProp::Rest(rest) => extract_binding_from_pat(&rest.arg)
            .is_some_and(|(sym, _)| is_likely_generated_alias(&sym)),
        _ => false,
    })
}

fn has_destructuring_candidates_in_params(params: &[Param]) -> bool {
    params.iter().any(|p| has_short_obj_pat_alias(&p.pat))
}

fn has_destructuring_candidates_in_stmts(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            return false;
        };
        var.decls
            .iter()
            .any(|decl| has_short_obj_pat_alias(&decl.name))
    })
}

fn destructuring_rename_function(func: &mut Function) {
    let Some(body) = &func.body else { return };
    if !has_destructuring_candidates_in_params(&func.params)
        && !has_destructuring_candidates_in_stmts(&body.stmts)
    {
        return;
    }
    let mut all_names = collect_names_in_stmts(&body.stmts);
    for p in &func.params {
        collect_names_in_pat(&p.pat, &mut all_names);
    }

    // Collect renames from both params and body VarDecls.
    // Feed param-rename targets into all_names so body renames don't
    // pick names that would shadow a just-renamed parameter.
    let mut renames = collect_obj_pat_renames_from_params(&func.params, &all_names);
    for r in &renames {
        all_names.insert(r.new.clone());
    }
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

fn destructuring_rename_module_with(module: &mut Module, all_names: &mut HashSet<Atom>) {
    let renames = collect_obj_pat_renames_from_module(&module.body, all_names);
    if renames.is_empty() {
        return;
    }
    for r in &renames {
        all_names.insert(r.new.clone());
    }
    rename_bindings_in_module(module, &renames);
    let mut shorthand = ObjectPatShorthandConverter;
    module.visit_mut_with(&mut shorthand);
}

fn destructuring_rename_arrow(arrow: &mut ArrowExpr) {
    let has_param_candidates = arrow.params.iter().any(has_short_obj_pat_alias);
    let has_body_candidates = match arrow.body.as_ref() {
        BlockStmtOrExpr::BlockStmt(b) => has_destructuring_candidates_in_stmts(&b.stmts),
        _ => false,
    };
    if !has_param_candidates && !has_body_candidates {
        return;
    }
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
    for r in &renames {
        all_names.insert(r.new.clone());
    }
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
    all_names: &HashSet<Atom>,
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
    all_names: &HashSet<Atom>,
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
    all_names: &HashSet<Atom>,
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
    all_names: &HashSet<Atom>,
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
    used_names: &mut HashSet<Atom>,
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
                if !is_likely_generated_alias(&alias.0) {
                    continue;
                }
                if alias.0.as_ref() == target_name {
                    continue;
                }
                if to_valid_identifier_name(&target_name) == alias.0.as_ref() {
                    continue;
                }
                let new_name = find_non_conflicting_name(&target_name, used_names);
                used_names.insert(Atom::from(new_name.as_str()));
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
                if !is_likely_generated_alias(&alias.0) {
                    continue;
                }
                let new_name = find_non_conflicting_name("rest", used_names);
                if new_name == alias.0.as_ref() {
                    continue;
                }
                used_names.insert(Atom::from(new_name.as_str()));
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

fn find_non_conflicting_name(base: &str, used_names: &HashSet<Atom>) -> String {
    let base = to_valid_identifier_name(base);

    let base_atom = Atom::from(base.as_str());
    if !used_names.contains(&base_atom) {
        return base;
    }
    for i in 1..=MAX_SYNTHETIC_NAME_ATTEMPTS {
        let candidate = format!("{}_{}", base, i);
        let candidate_atom = Atom::from(candidate.as_str());
        if !used_names.contains(&candidate_atom) {
            return candidate;
        }
    }
    panic!(
        "could not find non-conflicting name for `{base}` after {MAX_SYNTHETIC_NAME_ATTEMPTS} attempts"
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

fn has_member_init_candidates_in_stmts(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            return false;
        };
        var.decls.iter().any(|decl| {
            let Pat::Ident(bi) = &decl.name else {
                return false;
            };
            if !is_likely_generated_alias(&bi.id.sym) {
                return false;
            }
            matches!(
                decl.init.as_deref(),
                Some(Expr::Member(m)) if matches!(&m.prop, MemberProp::Ident(_))
            )
        })
    })
}

fn member_init_rename_function(func: &mut Function) {
    let Some(body) = &mut func.body else { return };
    if !has_member_init_candidates_in_stmts(&body.stmts) {
        return;
    }
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
    if !has_member_init_candidates_in_stmts(&block.stmts) {
        return;
    }
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

fn member_init_rename_module_with(module: &mut Module, all_names: &mut HashSet<Atom>) {
    let renames = collect_member_init_renames_from_module(&module.body, all_names);
    if renames.is_empty() {
        return;
    }
    for r in &renames {
        all_names.insert(r.new.clone());
    }
    rename_bindings_in_module(module, &renames);
}

fn collect_member_init_renames_from_module(
    body: &[ModuleItem],
    all_names: &HashSet<Atom>,
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
    all_names: &HashSet<Atom>,
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
    used_names: &mut HashSet<Atom>,
) {
    for decl in &var.decls {
        let Pat::Ident(bi) = &decl.name else { continue };
        let Some(init) = &decl.init else { continue };
        let old_name = bi.id.sym.to_string();

        // Only rename short (minified) names
        if !is_likely_generated_alias(old_name.as_str()) {
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
            if obj_name.chars().count() <= 2 && prop_name.chars().count() <= 2 {
                continue;
            }
            format!("{}_{}", obj_name, prop_name)
        } else {
            // Non-ident obj (e.g. call().prop) — skip if prop is too short
            if prop_name.chars().count() <= 2 {
                continue;
            }
            prop_name.clone()
        };

        if to_valid_identifier_name(&new_name) == old_name {
            continue;
        }
        let new_name = find_non_conflicting_name(&new_name, used_names);
        if new_name == old_name {
            continue;
        }
        used_names.insert(Atom::from(new_name.as_str()));
        renames.push(BindingRename {
            old: (bi.id.sym.clone(), bi.id.ctxt),
            new: new_name.as_str().into(),
        });
    }
}

// ============================================================
// Symbol.for("key") renames: var x = Symbol.for("react.element") → symbol_react_element
// ============================================================

fn has_symbol_for_candidates_in_stmts(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| {
        let Stmt::Decl(Decl::Var(var)) = stmt else { return false };
        var.decls.iter().any(|decl| {
            let Pat::Ident(bi) = &decl.name else { return false };
            if !is_likely_generated_alias(&bi.id.sym) {
                return false;
            }
            let Some(Expr::Call(call)) = decl.init.as_deref() else { return false };
            let Callee::Expr(callee) = &call.callee else { return false };
            matches!(callee.as_ref(), Expr::Member(MemberExpr { prop: MemberProp::Ident(prop), .. }) if prop.sym.as_ref() == "for")
        })
    })
}

fn symbol_for_rename_function(func: &mut Function, unresolved_mark: Mark) {
    let Some(body) = &mut func.body else { return };
    if !has_symbol_for_candidates_in_stmts(&body.stmts) {
        return;
    }
    let mut all_names = collect_names_in_stmts(&body.stmts);
    for p in &func.params {
        collect_names_in_pat(&p.pat, &mut all_names);
    }
    let renames = collect_symbol_for_renames_from_stmts(&body.stmts, &all_names, unresolved_mark);
    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut body.stmts, &renames);
}

fn symbol_for_rename_module_with(
    module: &mut Module,
    all_names: &mut HashSet<Atom>,
    unresolved_mark: Mark,
) {
    let renames = collect_symbol_for_renames_from_module(&module.body, all_names, unresolved_mark);
    if renames.is_empty() {
        return;
    }
    for r in &renames {
        all_names.insert(r.new.clone());
    }
    rename_bindings_in_module(module, &renames);
}

fn symbol_for_rename_arrow(arrow: &mut ArrowExpr, unresolved_mark: Mark) {
    let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_mut() else {
        return;
    };
    if !has_symbol_for_candidates_in_stmts(&block.stmts) {
        return;
    }
    let mut all_names = collect_names_in_stmts(&block.stmts);
    for p in &arrow.params {
        collect_names_in_pat(p, &mut all_names);
    }
    let all_names = all_names;
    let renames = collect_symbol_for_renames_from_stmts(&block.stmts, &all_names, unresolved_mark);
    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut block.stmts, &renames);
}

fn collect_symbol_for_renames_from_module(
    body: &[ModuleItem],
    all_names: &HashSet<Atom>,
    unresolved_mark: Mark,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();
    for item in body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                collect_symbol_for_var_renames(var, &mut renames, &mut used_names, unresolved_mark);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ed)) => {
                if let Decl::Var(var) = &ed.decl {
                    collect_symbol_for_var_renames(
                        var,
                        &mut renames,
                        &mut used_names,
                        unresolved_mark,
                    );
                }
            }
            _ => {}
        }
    }
    renames
}

fn collect_symbol_for_renames_from_stmts(
    stmts: &[Stmt],
    all_names: &HashSet<Atom>,
    unresolved_mark: Mark,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        collect_symbol_for_var_renames(var, &mut renames, &mut used_names, unresolved_mark);
    }
    renames
}

fn collect_symbol_for_var_renames(
    var: &VarDecl,
    renames: &mut Vec<BindingRename>,
    used_names: &mut HashSet<Atom>,
    unresolved_mark: Mark,
) {
    for decl in &var.decls {
        let Pat::Ident(bi) = &decl.name else { continue };
        let Some(init) = &decl.init else { continue };
        let old_name = bi.id.sym.to_string();

        // Only rename short (minified) names
        if !is_likely_generated_alias(old_name.as_str()) {
            continue;
        }

        // Match: Symbol.for("string")
        let Some(key) = extract_symbol_for_key(init, unresolved_mark) else {
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
        used_names.insert(Atom::from(new_name.as_str()));
        renames.push(BindingRename {
            old: (bi.id.sym.clone(), bi.id.ctxt),
            new: new_name.as_str().into(),
        });
    }
}

/// Extract the string key from `Symbol.for("key")`.
fn extract_symbol_for_key(expr: &Expr, unresolved_mark: Mark) -> Option<String> {
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
    if !is_unresolved_ident(obj_id, "Symbol", unresolved_mark) {
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

fn collect_names_in_module(body: &[ModuleItem]) -> HashSet<Atom> {
    let mut collector = NameCollector::default();
    body.visit_with(&mut collector);
    collector.names
}

fn collect_names_in_stmts(stmts: &[Stmt]) -> HashSet<Atom> {
    let mut collector = NameCollector::default();
    stmts.visit_with(&mut collector);
    collector.names
}

fn collect_names_in_expr(expr: &Expr, names: &mut HashSet<Atom>) {
    let mut collector = NameCollector::default();
    expr.visit_with(&mut collector);
    names.extend(collector.names);
}

fn collect_names_in_pat(pat: &Pat, names: &mut HashSet<Atom>) {
    let mut collector = NameCollector::default();
    pat.visit_with(&mut collector);
    names.extend(collector.names);
}

#[derive(Default)]
struct NameCollector {
    names: HashSet<Atom>,
}

impl Visit for NameCollector {
    fn visit_ident(&mut self, id: &Ident) {
        self.names.insert(id.sym.clone());
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
    let chars = key.chars().collect::<Vec<_>>();
    let mut result = String::new();
    let mut prev_was_sep = true; // treat start as after separator
    for (idx, ch) in chars.iter().enumerate() {
        if *ch == '.' || *ch == '-' || *ch == '_' || *ch == ' ' {
            if !result.is_empty() && !result.ends_with('_') {
                result.push('_');
            }
            prev_was_sep = true;
            continue;
        }

        let prev = idx.checked_sub(1).and_then(|prev_idx| chars.get(prev_idx));
        let next = chars.get(idx + 1);
        let camel_boundary = ch.is_ascii_uppercase()
            && !prev_was_sep
            && !result.is_empty()
            && (prev.is_some_and(|prev| prev.is_ascii_lowercase() || prev.is_ascii_digit())
                || (prev.is_some_and(|prev| prev.is_ascii_uppercase())
                    && next.is_some_and(|next| next.is_ascii_lowercase())));

        if camel_boundary {
            // camelCase/acronym boundary: "forwardRef" -> "FORWARD_REF", "URLValue" -> "URL_VALUE".
            result.push('_');
        }
        result.push(ch.to_ascii_uppercase());
        prev_was_sep = false;
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
    let renames = collect_value_position_renames_module(module);
    if renames.is_empty() {
        return;
    }
    rename_bindings_in_module(module, &renames);
    // Collapse `{ Foo: Foo }` created by the rename back to `{ Foo }`.
    module.visit_mut_with(&mut ObjShorthand);
}

fn collect_value_position_rename_map_module(module: &Module) -> HashMap<BindingId, String> {
    collect_value_position_renames_module(module)
        .into_iter()
        .map(|rename| (rename.old, rename.new.to_string()))
        .collect()
}

fn collect_value_position_renames_module(module: &Module) -> Vec<BindingRename> {
    let mut collector = BindingCollector::default();
    module.visit_with(&mut collector);
    if collector.short_bindings.is_empty() {
        return Vec::new();
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

    // Build the shadow index once for all candidates instead of per-candidate.
    let all_candidate_bids: HashSet<BindingId> =
        candidates.iter().map(|(_, bid)| bid.clone()).collect();
    let shadow_index = RenameShadowIndex::for_bindings(module, &all_candidate_bids);

    // Two-pass assignment: first reserve direct (unsuffixed) target names so
    // a later suffix fallback never steals another binding's natural target.
    let mut renames: Vec<BindingRename> = Vec::new();
    let mut committed_names: HashSet<Atom> = HashSet::new();
    let mut needs_suffix: Vec<(String, BindingId)> = Vec::new();

    for (target, bid) in candidates {
        if is_reserved_binding_name(&target) {
            continue;
        }
        let atom: Atom = target.as_str().into();
        if !top_level_names.contains(&atom) && !shadow_index.rename_causes_shadowing(&bid, &atom) {
            committed_names.insert(atom.clone());
            renames.push(BindingRename {
                old: bid,
                new: atom,
            });
        } else {
            needs_suffix.push((target, bid));
        }
    }

    for (target, bid) in needs_suffix {
        let final_name = (1..=10).map(|i| format!("{target}_{i}")).find(|candidate| {
            let atom: Atom = candidate.as_str().into();
            !committed_names.contains(&atom)
                && !top_level_names.contains(&atom)
                && !shadow_index.rename_causes_shadowing(&bid, &atom)
        });

        if let Some(name) = final_name {
            committed_names.insert(Atom::from(name.as_str()));
            renames.push(BindingRename {
                old: bid,
                new: name.as_str().into(),
            });
        }
    }

    if renames.is_empty() {
        return Vec::new();
    }
    renames
}

#[derive(Default)]
struct BindingCollector {
    short_bindings: HashMap<BindingId, ()>,
}

impl BindingCollector {
    fn record(&mut self, id: &Ident) {
        if is_likely_generated_alias(&id.sym) {
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

    fn visit_jsx_attr_or_spread(&mut self, attr: &JSXAttrOrSpread) {
        // Treat `<Foo name={x} />` the same as `{ name: x }` for
        // value-position renaming so JSX attrs also provide rename hints.
        let JSXAttrOrSpread::JSXAttr(JSXAttr {
            name: JSXAttrName::Ident(name),
            value:
                Some(JSXAttrValue::JSXExprContainer(JSXExprContainer {
                    expr: JSXExpr::Expr(expr),
                    ..
                })),
            ..
        }) = attr
        else {
            attr.visit_children_with(self);
            return;
        };
        if let Expr::Ident(id) = expr.as_ref() {
            let bid = (id.sym.clone(), id.ctxt);
            if self.states.contains_key(&bid) {
                let target = name.sym.to_string();
                if is_valid_js_ident(&target) && !is_reserved_binding_name(&target) {
                    self.record_value_use(&bid, target);
                } else {
                    self.record_other_use(&bid);
                }
                return;
            }
        }
        expr.visit_with(self);
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
    if raw.is_empty() || !is_valid_js_ident(&raw) || is_reserved_binding_name(&raw) {
        return None;
    }
    Some(raw)
}

// ============================================================
// Sentry component annotation rename: Sentry's Babel plugin
// (`@sentry/babel-plugin-component-annotate`) injects
// `data-sentry-component="OriginalName"` onto JSX elements.
// When the enclosing function has a minified name, use the
// annotation to recover the original component name.
// ============================================================

const SENTRY_ATTR_NAMES: &[&str] = &["data-sentry-component", "dataSentryComponent"];
const SENTRY_ELEMENT_ATTR_NAMES: &[&str] = &["data-sentry-element", "dataSentryElement"];
const SENTRY_SOURCE_FILE_ATTR_NAMES: &[&str] = &["data-sentry-source-file", "dataSentrySourceFile"];

fn sentry_component_rename_module(module: &mut Module) {
    let mut collector = SentryComponentCollector::default();
    module.visit_with(&mut collector);
    if collector.component_candidates.is_empty() && collector.element_candidates.is_empty() {
        return;
    }

    let mut used_names = collect_module_names(module);
    let component_candidate_bids: HashSet<BindingId> = collector
        .component_candidates
        .iter()
        .map(|(bid, _)| bid.clone())
        .collect();
    let mut candidates = collector.component_candidates;
    candidates.extend(
        collector
            .element_candidates
            .into_iter()
            .filter(|(bid, _)| !component_candidate_bids.contains(bid)),
    );

    let all_candidate_bids: HashSet<BindingId> =
        candidates.iter().map(|(bid, _)| bid.clone()).collect();
    let shadow_index = RenameShadowIndex::for_bindings(module, &all_candidate_bids);

    let mut renames = Vec::new();
    for (bid, target) in candidates {
        if bid.0.as_ref() == target.as_str() {
            continue;
        }
        if !is_likely_generated_alias(&bid.0) {
            continue;
        }
        if !is_valid_js_ident(&target) {
            continue;
        }
        if !target.starts_with(|c: char| c.is_ascii_uppercase()) {
            continue;
        }
        let atom: Atom = target.as_str().into();
        if used_names.contains(&atom) {
            continue;
        }
        if shadow_index.rename_causes_shadowing(&bid, &atom) {
            continue;
        }
        used_names.insert(atom.clone());
        renames.push(BindingRename {
            old: bid,
            new: atom,
        });
    }

    if !renames.is_empty() {
        rename_bindings_in_module(module, &renames);
    }
}

#[derive(Default)]
struct SentryComponentCollector {
    current_fn_binding: Option<BindingId>,
    component_candidates: Vec<(BindingId, String)>,
    element_candidates: Vec<(BindingId, String)>,
}

impl SentryComponentCollector {
    fn extract_sentry_attr_value(attrs: &[JSXAttrOrSpread], names: &[&str]) -> Option<String> {
        for attr in attrs {
            let JSXAttrOrSpread::JSXAttr(JSXAttr {
                name: JSXAttrName::Ident(name),
                value: Some(JSXAttrValue::Str(s)),
                ..
            }) = attr
            else {
                continue;
            };
            if names.contains(&name.sym.as_ref()) {
                if let Some(val) = s.value.as_str() {
                    if !val.is_empty() {
                        return Some(val.to_string());
                    }
                }
            }
        }
        None
    }

    fn extract_sentry_component_name(attrs: &[JSXAttrOrSpread]) -> Option<String> {
        Self::extract_sentry_attr_value(attrs, SENTRY_ATTR_NAMES)
    }

    fn extract_sentry_element_name(attrs: &[JSXAttrOrSpread]) -> Option<String> {
        let name = Self::extract_sentry_attr_value(attrs, SENTRY_ELEMENT_ATTR_NAMES)?;
        if let Some(source_file) =
            Self::extract_sentry_attr_value(attrs, SENTRY_SOURCE_FILE_ATTR_NAMES)
        {
            let source_name = sentry_source_file_component_name(&source_file)?;
            if source_name != name {
                return None;
            }
        }
        Some(name)
    }
}

fn sentry_source_file_component_name(source_file: &str) -> Option<String> {
    let file_name = source_file
        .rsplit(|ch| ch == '/' || ch == '\\')
        .next()
        .unwrap_or(source_file);
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem);
    let name = pascalize(stem);
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

impl Visit for SentryComponentCollector {
    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        let prev = self.current_fn_binding.take();
        self.current_fn_binding = Some((decl.ident.sym.clone(), decl.ident.ctxt));
        decl.function.visit_with(self);
        self.current_fn_binding = prev;
    }

    fn visit_var_declarator(&mut self, declarator: &swc_core::ecma::ast::VarDeclarator) {
        let Pat::Ident(binding) = &declarator.name else {
            declarator.visit_children_with(self);
            return;
        };
        let Some(init) = &declarator.init else {
            return;
        };
        match init.as_ref() {
            Expr::Fn(_) | Expr::Arrow(_) => {
                let prev = self.current_fn_binding.take();
                self.current_fn_binding = Some((binding.id.sym.clone(), binding.id.ctxt));
                init.visit_with(self);
                self.current_fn_binding = prev;
            }
            _ => {
                declarator.visit_children_with(self);
            }
        }
    }

    fn visit_jsx_opening_element(&mut self, elem: &swc_core::ecma::ast::JSXOpeningElement) {
        if let Some(bid) = &self.current_fn_binding {
            if let Some(name) = Self::extract_sentry_component_name(&elem.attrs) {
                self.component_candidates.push((bid.clone(), name));
            } else if let Some(name) = Self::extract_sentry_element_name(&elem.attrs) {
                self.element_candidates.push((bid.clone(), name));
            }
        }
        elem.visit_children_with(self);
    }

    fn visit_export_default_decl(&mut self, decl: &swc_core::ecma::ast::ExportDefaultDecl) {
        match &decl.decl {
            swc_core::ecma::ast::DefaultDecl::Fn(fn_expr) => {
                if let Some(ident) = &fn_expr.ident {
                    let prev = self.current_fn_binding.take();
                    self.current_fn_binding = Some((ident.sym.clone(), ident.ctxt));
                    fn_expr.function.visit_with(self);
                    self.current_fn_binding = prev;
                } else {
                    decl.visit_children_with(self);
                }
            }
            _ => decl.visit_children_with(self),
        }
    }
}

// ============================================================
// React function shape renames
//
// When a generated function binding already looks React-specific, recover a
// minimal readable name without guessing the original source name:
//
//   function K() { return <div />; }       -> function KComponent() { ... }
//   function K() { useEffect(...); }       -> function useK() { ... }
//
// Sentry component annotations remain higher priority. If a candidate contains
// a Sentry hint that was not accepted by `sentry_component_rename_module`, leave
// the function alone instead of falling back to a synthetic name.
// ============================================================

#[derive(Clone, Copy, Eq, PartialEq)]
enum ReactFunctionShapeKind {
    Component,
    Hook,
}

fn react_function_shape_rename_module(
    module: &mut Module,
    extracted_function_names: &HashMap<BindingId, Atom>,
) {
    let mut collector = ReactFunctionShapeCollector::new(extracted_function_names);
    module.visit_with(&mut collector);
    if collector.candidates.is_empty() {
        return;
    }

    let exported_bindings = collect_exported_binding_ids(module);
    let component_use_bindings = collect_component_use_binding_ids(module);
    let mut used_names = collect_module_names(module);
    let all_candidate_bids: HashSet<BindingId> = collector
        .candidates
        .iter()
        .map(|(bid, _)| bid.clone())
        .collect();
    let shadow_index = RenameShadowIndex::for_bindings(module, &all_candidate_bids);

    let mut renames = Vec::new();
    for (bid, kind) in collector.candidates {
        if exported_bindings.contains(&bid) {
            continue;
        }
        let is_extracted_function = extracted_function_names.contains_key(&bid);
        if !is_likely_generated_alias(&bid.0) && !is_extracted_function {
            continue;
        }
        let kind = if component_use_bindings.contains(&bid) {
            ReactFunctionShapeKind::Component
        } else {
            kind
        };
        let base_name = extracted_function_names
            .get(&bid)
            .map_or_else(|| bid.0.as_ref(), Atom::as_ref);
        let target = react_function_shape_target_name(base_name, kind);
        if target == bid.0.as_ref() || !is_valid_js_ident(&target) {
            continue;
        }

        let atom: Atom = target.as_str().into();
        if used_names.contains(&atom) || shadow_index.rename_causes_shadowing(&bid, &atom) {
            continue;
        }

        used_names.insert(atom.clone());
        renames.push(BindingRename {
            old: bid,
            new: atom,
        });
    }

    if !renames.is_empty() {
        rename_bindings_in_module(module, &renames);
    }
}

fn react_function_shape_target_name(name: &str, kind: ReactFunctionShapeKind) -> String {
    let base = pascalize(name);
    match kind {
        ReactFunctionShapeKind::Component if base == "Component" => base,
        ReactFunctionShapeKind::Component => format!("{base}Component"),
        ReactFunctionShapeKind::Hook => format!("use{base}"),
    }
}

struct ReactFunctionShapeCollector<'a> {
    candidates: Vec<(BindingId, ReactFunctionShapeKind)>,
    extracted_function_names: &'a HashMap<BindingId, Atom>,
}

impl<'a> ReactFunctionShapeCollector<'a> {
    fn new(extracted_function_names: &'a HashMap<BindingId, Atom>) -> Self {
        Self {
            candidates: Vec::new(),
            extracted_function_names,
        }
    }

    fn record_function(&mut self, id: &Ident, function: &Function) {
        let bid = (id.sym.clone(), id.ctxt);
        if !is_likely_generated_alias(&id.sym) && !self.extracted_function_names.contains_key(&bid)
        {
            return;
        }
        if let Some(kind) = classify_react_function(function) {
            self.candidates.push((bid, kind));
        }
    }

    fn record_arrow(&mut self, id: &Ident, arrow: &ArrowExpr) {
        let bid = (id.sym.clone(), id.ctxt);
        if !is_likely_generated_alias(&id.sym) && !self.extracted_function_names.contains_key(&bid)
        {
            return;
        }
        if let Some(kind) = classify_react_arrow(arrow) {
            self.candidates.push((bid, kind));
        }
    }
}

impl Visit for ReactFunctionShapeCollector<'_> {
    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.record_function(&decl.ident, &decl.function);
        decl.function.visit_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &swc_core::ecma::ast::VarDeclarator) {
        let Pat::Ident(binding) = &declarator.name else {
            declarator.visit_children_with(self);
            return;
        };
        let Some(init) = &declarator.init else {
            return;
        };
        match init.as_ref() {
            Expr::Fn(fn_expr) => {
                self.record_function(&binding.id, &fn_expr.function);
                fn_expr.function.visit_with(self);
            }
            Expr::Arrow(arrow) => {
                self.record_arrow(&binding.id, arrow);
                arrow.visit_with(self);
            }
            _ => declarator.visit_children_with(self),
        }
    }

    fn visit_export_default_decl(&mut self, decl: &swc_core::ecma::ast::ExportDefaultDecl) {
        match &decl.decl {
            swc_core::ecma::ast::DefaultDecl::Fn(fn_expr) => {
                if let Some(ident) = &fn_expr.ident {
                    self.record_function(ident, &fn_expr.function);
                }
                fn_expr.function.visit_with(self);
            }
            _ => decl.visit_children_with(self),
        }
    }

    fn visit_fn_expr(&mut self, fn_expr: &FnExpr) {
        fn_expr.function.visit_with(self);
    }
}

fn classify_react_function(function: &Function) -> Option<ReactFunctionShapeKind> {
    let mut classifier = ReactFunctionBodyClassifier::default();
    function.visit_with(&mut classifier);
    classifier.kind()
}

fn classify_react_arrow(arrow: &ArrowExpr) -> Option<ReactFunctionShapeKind> {
    let mut classifier = ReactFunctionBodyClassifier::default();
    arrow.body.visit_with(&mut classifier);
    classifier.kind()
}

#[derive(Default)]
struct ReactFunctionBodyClassifier {
    has_jsx: bool,
    has_hook_call: bool,
    has_sentry_hint: bool,
}

impl ReactFunctionBodyClassifier {
    fn kind(&self) -> Option<ReactFunctionShapeKind> {
        if self.has_sentry_hint {
            return None;
        }
        if self.has_jsx {
            return Some(ReactFunctionShapeKind::Component);
        }
        if self.has_hook_call {
            return Some(ReactFunctionShapeKind::Hook);
        }
        None
    }
}

impl Visit for ReactFunctionBodyClassifier {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(name) = callee_terminal_name(&call.callee) {
            if is_react_hook_name(&name) {
                self.has_hook_call = true;
            }
        }
        call.visit_children_with(self);
    }

    fn visit_jsx_element(&mut self, elem: &swc_core::ecma::ast::JSXElement) {
        self.has_jsx = true;
        elem.visit_children_with(self);
    }

    fn visit_jsx_fragment(&mut self, fragment: &swc_core::ecma::ast::JSXFragment) {
        self.has_jsx = true;
        fragment.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_jsx_opening_element(&mut self, elem: &swc_core::ecma::ast::JSXOpeningElement) {
        if elem.attrs.iter().any(|attr| {
            matches!(
                attr,
                JSXAttrOrSpread::JSXAttr(JSXAttr {
                    name: JSXAttrName::Ident(name),
                    ..
                }) if is_sentry_hint_attr_name(name.sym.as_ref())
            )
        }) {
            self.has_sentry_hint = true;
        }
        elem.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, _: &FnDecl) {}

    fn visit_fn_expr(&mut self, _: &FnExpr) {}

    fn visit_class_decl(&mut self, _: &ClassDecl) {}

    fn visit_class_expr(&mut self, _: &ClassExpr) {}
}

fn is_sentry_hint_attr_name(name: &str) -> bool {
    SENTRY_ATTR_NAMES.contains(&name)
        || SENTRY_ELEMENT_ATTR_NAMES.contains(&name)
        || SENTRY_SOURCE_FILE_ATTR_NAMES.contains(&name)
}

fn callee_terminal_name(callee: &Callee) -> Option<String> {
    match callee {
        Callee::Expr(expr) => match expr.as_ref() {
            Expr::Ident(id) => Some(id.sym.to_string()),
            Expr::Member(member) => static_member_prop_name(&member.prop).map(String::from),
            _ => None,
        },
        _ => None,
    }
}

fn is_react_hook_name(name: &str) -> bool {
    matches!(
        name,
        "useState"
            | "useEffect"
            | "useLayoutEffect"
            | "useInsertionEffect"
            | "useMemo"
            | "useCallback"
            | "useRef"
            | "useContext"
            | "useReducer"
            | "useImperativeHandle"
            | "useDebugValue"
            | "useDeferredValue"
            | "useTransition"
            | "useId"
            | "useSyncExternalStore"
            | "useOptimistic"
            | "useActionState"
            | "useFormStatus"
    )
}

fn collect_component_use_binding_ids(module: &Module) -> HashSet<BindingId> {
    let mut collector = ComponentUseBindingCollector::default();
    module.visit_with(&mut collector);
    collector.bindings
}

#[derive(Default)]
struct ComponentUseBindingCollector {
    bindings: HashSet<BindingId>,
}

impl Visit for ComponentUseBindingCollector {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if callee_terminal_name(&call.callee).as_deref() == Some("createElement") {
            if let Some(first_arg) = call.args.first() {
                if let Expr::Ident(ident) = first_arg.expr.as_ref() {
                    self.bindings.insert((ident.sym.clone(), ident.ctxt));
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_jsx_element_name(&mut self, name: &JSXElementName) {
        match name {
            JSXElementName::Ident(ident) => {
                self.bindings.insert((ident.sym.clone(), ident.ctxt));
            }
            JSXElementName::JSXMemberExpr(member) => self.visit_jsx_member_expr(member),
            JSXElementName::JSXNamespacedName(_) => {}
        }
    }

    fn visit_jsx_member_expr(&mut self, member: &JSXMemberExpr) {
        match &member.obj {
            JSXObject::Ident(ident) => {
                self.bindings.insert((ident.sym.clone(), ident.ctxt));
            }
            JSXObject::JSXMemberExpr(member) => self.visit_jsx_member_expr(member),
        }
    }
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
        if collector
            .all_binding_names
            .contains(&Atom::from(state.target.as_str()))
        {
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
    all_binding_names: HashSet<Atom>,
}

impl JsxComponentAliasCollector {
    fn record_binding_name(&mut self, id: &Ident) {
        self.all_binding_names.insert(id.sym.clone());
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
            if !is_likely_generated_alias(&binding.id.sym) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_non_conflicting_name_uses_next_available_suffix() {
        let used_names = HashSet::from([
            Atom::from("rest"),
            Atom::from("rest_1"),
            Atom::from("rest_2"),
        ]);

        assert_eq!(find_non_conflicting_name("rest", &used_names), "rest_3");
    }
}
