use crate::collections::{HashMap, HashSet};
use std::cell::RefCell;
use std::rc::Rc;

use swc_core::atoms::Atom;
use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, ArrowExpr, ArrowFunctionBody, AssignPatProp, BindingIdent, BlockStmt, CallExpr,
    Callee, Class, ClassDecl, ClassExpr, Constructor, Decl, Expr, FnDecl, FnExpr, ForHead,
    ForInStmt, ForOfStmt, ForStmt, Function, GetterProp, Ident, ImportDecl, ImportSpecifier,
    JSXAttr, JSXAttrName, JSXAttrOrSpread, JSXAttrValue, JSXElementName, JSXExpr, JSXExprContainer,
    JSXMemberExpr, JSXObject, KeyValuePatProp, Lit, MemberExpr, MemberProp, Module, ModuleDecl,
    ModuleItem, ObjectPat, ObjectPatProp, ParamOrTsParamProp, Pat, Prop, PropName, Stmt,
    SwitchStmt, VarDecl, VarDeclKind, VarDeclOrExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::js_names::{
    is_likely_generated_alias, is_reserved_binding_name, to_valid_identifier_name,
};

use super::eval_utils::{
    has_dynamic_scope_construct, js_source_mentions_binding, module_has_with_stmt,
    DirectEvalAnalyzer,
};
use super::expr_utils::is_unresolved_ident;
use super::extract_inlined_function::SharedExtractedFunctionNames;
use super::helper_matcher::static_member_prop_name;
use super::rename_utils::{
    collect_exported_binding_ids, collect_jsx_tag_bindings, collect_module_names, rename_bindings,
    rename_bindings_in_module, starts_with_lowercase, BindingId, BindingRename, RenameShadowIndex,
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
            pending_value_position_names: HashMap::default(),
            extracted_function_names: Rc::new(RefCell::new(HashMap::default())),
        }
    }
}

impl VisitMut for SmartRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let previous_pending_names = std::mem::replace(
            &mut self.pending_value_position_names,
            collect_value_position_rename_map_module(module),
        );
        let exported_bindings = collect_exported_binding_ids(module);
        let mut cached_names = collect_names_in_module(&module.body);
        react_rename_module_with(
            module,
            &mut cached_names,
            &self.pending_value_position_names,
            &exported_bindings,
        );
        destructuring_rename_module_with(module, &mut cached_names, &exported_bindings);
        member_init_rename_module_with(module, &mut cached_names, &exported_bindings);
        import_snapshot_alias_rename_module(module, &mut cached_names);
        symbol_for_rename_module_with(
            module,
            &mut cached_names,
            self.unresolved_mark,
            &exported_bindings,
        );

        sentry_component_rename_module(module, &exported_bindings);
        react_function_shape_rename_module(module, &self.extracted_function_names.borrow());
        module.visit_mut_children_with(self);
        // Runs once at the module level; uses (sym, ctxt) matching so nested
        // bindings are classified correctly without per-scope recursion.
        let value_named = value_position_rename_module(module);
        call_site_param_rename_module(module, &value_named);
        role_rename_module(module, self.unresolved_mark);
        jsx_component_alias_rename_module(module, &exported_bindings);
        self.pending_value_position_names = previous_pending_names;
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        react_rename_function_body(func, &self.pending_value_position_names);
        destructuring_rename_function(func);
        member_init_rename_function(func);
        symbol_for_rename_function(func, self.unresolved_mark);
        func.visit_mut_children_with(self);
    }

    fn visit_mut_constructor(&mut self, ctor: &mut Constructor) {
        destructuring_rename_constructor(ctor);
        ctor.visit_mut_children_with(self);
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
            pending_value_position_names: HashMap::default(),
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
        let exported_bindings = collect_exported_binding_ids(module);
        sentry_component_rename_module(module, &exported_bindings);
        react_function_shape_rename_module(module, &self.extracted_function_names.borrow());
        module.visit_mut_children_with(self);
        value_position_rename_module(module);
        jsx_component_alias_rename_module(module, &exported_bindings);
        self.pending_value_position_names = previous_pending_names;
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        react_rename_function_body(func, &self.pending_value_position_names);
        destructuring_rename_function(func);
        member_init_rename_function(func);
        symbol_for_rename_function(func, self.unresolved_mark);
        func.visit_mut_children_with(self);
    }

    fn visit_mut_constructor(&mut self, ctor: &mut Constructor) {
        destructuring_rename_constructor(ctor);
        ctor.visit_mut_children_with(self);
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
    exported_bindings: &HashSet<BindingId>,
) {
    let mut renames = collect_react_renames_from_module_items(
        &module.body,
        all_names,
        pending_value_position_names,
    );
    renames.retain(|rename| !exported_bindings.contains(&rename.old));
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
    let ArrowFunctionBody::FunctionBody(body) = arrow.body.as_mut() else {
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
        "useActionState" => {
            // `const [state, dispatchAction, isPending] = useActionState(action, initialState)`
            rename_array_elem_if_short(array_pat, 0, "state", renames, used_names);
            rename_array_elem_if_short(array_pat, 1, "dispatchAction", renames, used_names);
            rename_array_elem_if_short(array_pat, 2, "isPending", renames, used_names);
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
        // useActionState(action, initialState, permalink?) - initialState is
        // required, so demand at least two args to avoid matching unrelated
        // one-argument calls that happen to share the name.
        "useActionState" => args.len() >= 2 && args.len() <= 3,
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

/// Whether `pat` holds a short object-pattern alias, including inside a
/// defaulted pattern (`{ a: b } = {}`), an array pattern (`[{ a: b }]`), or a
/// nested object pattern (`{ a: { b: c } }`).
fn has_short_obj_pat_alias(pat: &Pat) -> bool {
    match pat {
        Pat::Object(obj_pat) => obj_pat.props.iter().any(|prop| match prop {
            ObjectPatProp::KeyValue(kv) => {
                has_short_obj_pat_alias(&kv.value)
                    || extract_binding_from_pat(&kv.value)
                        .is_some_and(|(sym, _)| is_likely_generated_alias(&sym))
            }
            ObjectPatProp::Rest(rest) => extract_binding_from_pat(&rest.arg)
                .is_some_and(|(sym, _)| is_likely_generated_alias(&sym)),
            ObjectPatProp::Assign(_) => false,
        }),
        Pat::Array(array_pat) => array_pat
            .elems
            .iter()
            .flatten()
            .any(has_short_obj_pat_alias),
        Pat::Assign(assign_pat) => has_short_obj_pat_alias(&assign_pat.left),
        _ => false,
    }
}

/// Whether any declaration in `stmts` — at the top level, in a nested block,
/// or in a `for` head — has a short destructuring alias. Nested functions
/// and classes are skipped; their own visit handles them.
fn has_destructuring_candidates_in_stmts(stmts: &[Stmt]) -> bool {
    let mut finder = ObjPatAliasFinder::default();
    stmts.visit_with(&mut finder);
    finder.found
}

#[derive(Default)]
struct ObjPatAliasFinder {
    found: bool,
}

impl Visit for ObjPatAliasFinder {
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}

    fn visit_var_decl(&mut self, var: &VarDecl) {
        if var
            .decls
            .iter()
            .any(|decl| has_short_obj_pat_alias(&decl.name))
        {
            self.found = true;
        }
    }
}

fn direct_var_decls(stmts: &[Stmt]) -> impl Iterator<Item = &VarDecl> {
    stmts.iter().filter_map(|stmt| match stmt {
        Stmt::Decl(Decl::Var(var)) => Some(&**var),
        _ => None,
    })
}

/// Collects destructuring renames for declarations below the top level of a
/// function or module body: nested blocks (`if`/loop/`try`/`switch` bodies)
/// and `for` heads. The caller handles top-level declarations first and
/// passes their renames in; nested functions and classes are skipped.
///
/// A block-scoped binding can only capture names used inside its own scope,
/// so its conflict set is the names in that scope plus every new name visible
/// there: the caller's renames, enclosing scopes' renames, and nested `var`
/// renames. Disjoint blocks can therefore reuse the same name. A nested `var`
/// is function-scoped and checks the function-wide set instead. Every chosen
/// name joins the function-wide set, so a later `var` avoids it too.
struct NestedObjPatRenameCollector<'a> {
    function_names: &'a mut HashSet<Atom>,
    renames: &'a mut Vec<BindingRename>,
    /// New names visible in the current scope; truncated on scope exit.
    visible: Vec<Atom>,
    /// New names of nested `var` renames; visible everywhere.
    var_names: Vec<Atom>,
}

impl<'a> NestedObjPatRenameCollector<'a> {
    fn new(function_names: &'a mut HashSet<Atom>, renames: &'a mut Vec<BindingRename>) -> Self {
        for rename in renames.iter() {
            function_names.insert(rename.new.clone());
        }
        let visible = renames.iter().map(|rename| rename.new.clone()).collect();
        Self {
            function_names,
            renames,
            visible,
            var_names: Vec::new(),
        }
    }

    fn collect_scope<'d>(
        &mut self,
        decls: impl Iterator<Item = &'d VarDecl>,
        scope_names: impl FnOnce() -> HashSet<Atom>,
    ) {
        let mut block_scoped = Vec::new();
        for var in decls {
            if !var
                .decls
                .iter()
                .any(|decl| has_short_obj_pat_alias(&decl.name))
            {
                continue;
            }
            if var.kind != VarDeclKind::Var {
                block_scoped.push(var);
                continue;
            }
            let start = self.renames.len();
            for decl in &var.decls {
                collect_obj_pat_renames_from_pat(&decl.name, self.renames, self.function_names);
            }
            self.var_names.extend(
                self.renames[start..]
                    .iter()
                    .map(|rename| rename.new.clone()),
            );
        }
        if block_scoped.is_empty() {
            return;
        }
        let mut used = scope_names();
        used.extend(self.visible.iter().cloned());
        used.extend(self.var_names.iter().cloned());
        let start = self.renames.len();
        for var in block_scoped {
            for decl in &var.decls {
                collect_obj_pat_renames_from_pat(&decl.name, self.renames, &mut used);
            }
        }
        for rename in &self.renames[start..] {
            self.function_names.insert(rename.new.clone());
            self.visible.push(rename.new.clone());
        }
    }
}

fn names_in<N: VisitWith<NameCollector> + ?Sized>(node: &N) -> HashSet<Atom> {
    let mut collector = NameCollector::default();
    node.visit_with(&mut collector);
    collector.names
}

impl Visit for NestedObjPatRenameCollector<'_> {
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}

    fn visit_block_stmt(&mut self, block: &BlockStmt) {
        let mark = self.visible.len();
        self.collect_scope(direct_var_decls(&block.stmts), || names_in(block));
        block.visit_children_with(self);
        self.visible.truncate(mark);
    }

    fn visit_switch_stmt(&mut self, switch: &SwitchStmt) {
        let mark = self.visible.len();
        let decls = switch
            .cases
            .iter()
            .flat_map(|case| direct_var_decls(&case.cons));
        self.collect_scope(decls, || names_in(switch));
        switch.visit_children_with(self);
        self.visible.truncate(mark);
    }

    fn visit_for_stmt(&mut self, for_stmt: &ForStmt) {
        let mark = self.visible.len();
        if let Some(VarDeclOrExpr::VarDecl(var)) = &for_stmt.init {
            self.collect_scope(std::iter::once(&**var), || names_in(for_stmt));
        }
        for_stmt.visit_children_with(self);
        self.visible.truncate(mark);
    }

    fn visit_for_of_stmt(&mut self, for_of: &ForOfStmt) {
        let mark = self.visible.len();
        if let ForHead::VarDecl(var) = &for_of.left {
            self.collect_scope(std::iter::once(&**var), || names_in(for_of));
        }
        for_of.visit_children_with(self);
        self.visible.truncate(mark);
    }

    fn visit_for_in_stmt(&mut self, for_in: &ForInStmt) {
        let mark = self.visible.len();
        if let ForHead::VarDecl(var) = &for_in.left {
            self.collect_scope(std::iter::once(&**var), || names_in(for_in));
        }
        for_in.visit_children_with(self);
        self.visible.truncate(mark);
    }
}

fn destructuring_rename_function(func: &mut Function) {
    let Some(body) = &func.body else { return };
    let param_pats: Vec<&Pat> = func.params.iter().map(|p| &p.pat).collect();
    let renames =
        collect_function_destructuring_renames(&param_pats, names_in(&func.params), &body.stmts);
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

/// Constructors are not `Function` nodes, so `visit_mut_function` never sees
/// them. TypeScript parameter properties are left alone: their name is also
/// the property name.
fn destructuring_rename_constructor(ctor: &mut Constructor) {
    let Some(body) = &ctor.body else { return };
    let param_pats: Vec<&Pat> = ctor
        .params
        .iter()
        .filter_map(|p| match p {
            ParamOrTsParamProp::Param(param) => Some(&param.pat),
            ParamOrTsParamProp::TsParamProp(_) => None,
        })
        .collect();
    let renames =
        collect_function_destructuring_renames(&param_pats, names_in(&ctor.params), &body.stmts);
    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut ctor.params, &renames);
    if let Some(body) = &mut ctor.body {
        rename_bindings(&mut body.stmts, &renames);
    }
    let mut shorthand = ObjectPatShorthandConverter;
    ctor.params
        .iter_mut()
        .for_each(|p| p.visit_mut_with(&mut shorthand));
    if let Some(body) = &mut ctor.body {
        body.visit_mut_with(&mut shorthand);
    }
}

/// Destructuring renames for a function-like body: parameters first, then
/// top-level body declarations, then nested blocks and `for` heads.
/// `param_names` holds every name in the parameter list.
fn collect_function_destructuring_renames(
    param_pats: &[&Pat],
    param_names: HashSet<Atom>,
    stmts: &[Stmt],
) -> Vec<BindingRename> {
    if !param_pats.iter().any(|p| has_short_obj_pat_alias(p))
        && !has_destructuring_candidates_in_stmts(stmts)
    {
        return Vec::new();
    }
    let mut all_names = collect_names_in_stmts(stmts);
    all_names.extend(param_names);

    // Feed param-rename targets into all_names so body renames don't
    // pick names that would shadow a just-renamed parameter.
    let mut renames = Vec::new();
    let mut used_names = all_names.clone();
    for pat in param_pats {
        collect_obj_pat_renames_from_pat(pat, &mut renames, &mut used_names);
    }
    for r in &renames {
        all_names.insert(r.new.clone());
    }
    renames.extend(collect_obj_pat_renames_from_stmts(stmts, &all_names));
    stmts.visit_with(&mut NestedObjPatRenameCollector::new(
        &mut all_names,
        &mut renames,
    ));
    renames
}

fn destructuring_rename_module_with(
    module: &mut Module,
    all_names: &mut HashSet<Atom>,
    exported_bindings: &HashSet<BindingId>,
) {
    let mut renames = collect_obj_pat_renames_from_module(&module.body, all_names);
    renames.retain(|rename| !exported_bindings.contains(&rename.old));
    module
        .body
        .visit_with(&mut NestedObjPatRenameCollector::new(
            all_names,
            &mut renames,
        ));
    if renames.is_empty() {
        return;
    }
    rename_bindings_in_module(module, &renames);
    let mut shorthand = ObjectPatShorthandConverter;
    module.visit_mut_with(&mut shorthand);
}

fn destructuring_rename_arrow(arrow: &mut ArrowExpr) {
    let has_param_candidates = arrow.params.iter().any(has_short_obj_pat_alias);
    let has_body_candidates = match arrow.body.as_ref() {
        ArrowFunctionBody::FunctionBody(b) => has_destructuring_candidates_in_stmts(&b.stmts),
        _ => false,
    };
    if !has_param_candidates && !has_body_candidates {
        return;
    }
    let mut all_names = match arrow.body.as_ref() {
        ArrowFunctionBody::FunctionBody(b) => collect_names_in_stmts(&b.stmts),
        ArrowFunctionBody::Expr(e) => {
            let mut names = HashSet::default();
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
    if let ArrowFunctionBody::FunctionBody(b) = arrow.body.as_ref() {
        renames.extend(collect_obj_pat_renames_from_stmts(&b.stmts, &all_names));
        b.stmts.visit_with(&mut NestedObjPatRenameCollector::new(
            &mut all_names,
            &mut renames,
        ));
    }
    if renames.is_empty() {
        return;
    }
    rename_bindings(&mut arrow.params, &renames);
    match arrow.body.as_mut() {
        ArrowFunctionBody::FunctionBody(block) => {
            rename_bindings(&mut block.stmts, &renames);
            block.visit_mut_with(&mut ObjectPatShorthandConverter);
        }
        ArrowFunctionBody::Expr(expr) => rename_bindings(expr, &renames),
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
    let obj_pat = match pat {
        Pat::Object(obj_pat) => obj_pat,
        Pat::Assign(assign_pat) => {
            collect_obj_pat_renames_from_pat(&assign_pat.left, renames, used_names);
            return;
        }
        Pat::Array(array_pat) => {
            for elem in array_pat.elems.iter().flatten() {
                collect_obj_pat_renames_from_pat(elem, renames, used_names);
            }
            return;
        }
        _ => return,
    };
    for prop in &obj_pat.props {
        match prop {
            ObjectPatProp::KeyValue(kv) => {
                let nested = match &*kv.value {
                    Pat::Assign(assign_pat) => &*assign_pat.left,
                    value => value,
                };
                if matches!(nested, Pat::Object(_) | Pat::Array(_)) {
                    collect_obj_pat_renames_from_pat(nested, renames, used_names);
                    continue;
                }
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
    let ArrowFunctionBody::FunctionBody(block) = arrow.body.as_mut() else {
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

fn member_init_rename_module_with(
    module: &mut Module,
    all_names: &mut HashSet<Atom>,
    exported_bindings: &HashSet<BindingId>,
) {
    let mut renames = collect_member_init_renames_from_module(&module.body, all_names);
    renames.retain(|rename| !exported_bindings.contains(&rename.old));
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

/// Give a module-level snapshot of a named import a readable local name while
/// keeping the `const` copy intact. This deliberately does not inline the
/// imported binding: the provider may mutate its live export after this module
/// captures the value.
fn import_snapshot_alias_rename_module(module: &mut Module, all_names: &mut HashSet<Atom>) {
    if module_has_with_stmt(module) {
        return;
    }
    let mut direct_eval = DirectEvalAnalyzer::default();
    module.visit_with(&mut direct_eval);
    if direct_eval.unknown_direct_eval {
        return;
    }

    let mut named_imports = HashSet::default();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        for specifier in &import.specifiers {
            let ImportSpecifier::Named(named) = specifier else {
                continue;
            };
            named_imports.insert((named.local.sym.clone(), named.local.ctxt));
        }
    }
    if named_imports.is_empty() {
        return;
    }

    let jsx_tags = collect_jsx_tag_bindings(module);
    let mut renames = Vec::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        if var.kind != VarDeclKind::Const {
            continue;
        }
        for decl in &var.decls {
            let Pat::Ident(alias) = &decl.name else {
                continue;
            };
            if !is_likely_generated_alias(&alias.id.sym) {
                continue;
            }
            let Some(init) = &decl.init else { continue };
            let Expr::Ident(imported) = init.as_ref() else {
                continue;
            };
            if !named_imports.contains(&(imported.sym.clone(), imported.ctxt))
                || is_likely_generated_alias(&imported.sym)
            {
                continue;
            }
            let alias_id = (alias.id.sym.clone(), alias.id.ctxt);
            if starts_with_lowercase(&imported.sym) && jsx_tags.contains(&alias_id) {
                continue;
            }

            let target = find_non_conflicting_name(imported.sym.as_ref(), all_names);
            let target: Atom = target.into();
            if direct_eval.known_direct_eval_sources.iter().any(|source| {
                js_source_mentions_binding(source, &alias.id.sym)
                    || js_source_mentions_binding(source, &target)
            }) {
                continue;
            }
            all_names.insert(target.clone());
            renames.push(BindingRename {
                old: alias_id,
                new: target,
            });
        }
    }

    if !renames.is_empty() {
        rename_bindings_in_module(module, &renames);
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
    exported_bindings: &HashSet<BindingId>,
) {
    let mut renames =
        collect_symbol_for_renames_from_module(&module.body, all_names, unresolved_mark);
    renames.retain(|rename| !exported_bindings.contains(&rename.old));
    if renames.is_empty() {
        return;
    }
    for r in &renames {
        all_names.insert(r.new.clone());
    }
    rename_bindings_in_module(module, &renames);
}

fn symbol_for_rename_arrow(arrow: &mut ArrowExpr, unresolved_mark: Mark) {
    let ArrowFunctionBody::FunctionBody(block) = arrow.body.as_mut() else {
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
// A short binding `x` used as the value of object-literal KeyValue
// properties with a valid-identifier key, where every such key agrees on
// the same target name, is renamed to that name.
//
//   (e, t) => ({ ...e, error: t })      → (e, error) => ({ ...e, error })
//   import r from "m"; export default { Foo: r }
//                                        → import Foo from "m"; export default { Foo }
//
// Disqualified:
//   - Multiple distinct target names (e.g. `{ array: e, bool: e }`)
//   - Computed/numeric/reserved-keyword keys
//
// When the binding also has other uses (member access, call arg, writes,
// ...), the key names one destination of the value rather than the value
// itself more often, so these are disqualified too:
//   - Generic keys (`type`, `name`, `value`, `data`, `key`) and `$` keys
//   - Bindings declared inside a destructuring pattern
//   - Class declarations and class-valued declarators
//   - A boolean-valued initializer that tests a property with the key's
//     name (`const t = !!s.icon` → `{ icon: t }` is a flag about `icon`)
// ============================================================

/// Returns the renamed bindings under their new names.
fn value_position_rename_module(module: &mut Module) -> HashSet<BindingId> {
    let renames = collect_value_position_renames_module(module);
    if renames.is_empty() {
        return HashSet::default();
    }
    rename_bindings_in_module(module, &renames);
    // Collapse `{ Foo: Foo }` created by the rename back to `{ Foo }`.
    module.visit_mut_with(&mut ObjShorthand);
    renames
        .into_iter()
        .map(|rename| (rename.new, rename.old.1))
        .collect()
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
    // the key isn't discriminative — drop the whole group. Candidates that
    // also have other uses form a second tier: they only take a target no
    // sole-use candidate claims, so relaxing never costs a sole-use rename.
    let mut by_target: HashMap<String, Vec<BindingId>> = HashMap::default();
    let mut relaxed_by_target: HashMap<String, Vec<BindingId>> = HashMap::default();
    for (bid, state) in classifier.states {
        let Some(target) = state.single_target() else {
            continue;
        };
        if target.as_str() == bid.0.as_ref() {
            continue;
        }
        let tier = if state.other_uses > 0 {
            &mut relaxed_by_target
        } else {
            &mut by_target
        };
        tier.entry(target).or_default().push(bid);
    }
    for (target, bids) in relaxed_by_target {
        by_target.entry(target).or_insert(bids);
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
    let capture_sensitive_names: HashSet<Atom> = candidates
        .iter()
        .flat_map(|(target, _)| {
            std::iter::once(Atom::from(target.as_str()))
                .chain((1..=10).map(move |i| Atom::from(format!("{target}_{i}"))))
        })
        .collect();
    let shadow_index = RenameShadowIndex::for_bindings(module, &all_candidate_bids);
    let scope_name_index =
        BindingScopeNameIndex::for_bindings(module, &all_candidate_bids, &capture_sensitive_names);

    // Two-pass assignment: first reserve direct (unsuffixed) target names so
    // a later suffix fallback never steals another binding's natural target.
    let mut renames: Vec<BindingRename> = Vec::new();
    let mut committed_names: HashSet<Atom> = HashSet::default();
    let mut needs_suffix: Vec<(String, BindingId)> = Vec::new();

    for (target, bid) in candidates {
        if is_reserved_binding_name(&target) {
            continue;
        }
        let atom: Atom = target.as_str().into();
        if !top_level_names.contains(&atom)
            && !shadow_index.rename_causes_shadowing(&bid, &atom)
            && !scope_name_index.rename_would_capture(&bid, &atom)
        {
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
                && !scope_name_index.rename_would_capture(&bid, &atom)
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

/// Free emitted identifier names inside each candidate binding's lexical
/// scope. Names satisfied by a nested declaration stop at that scope; names
/// that propagate outward currently resolve to an outer binding or the global
/// scope and would be captured by a same-name candidate declaration. This is
/// the opposite direction from `RenameShadowIndex`, which protects the
/// candidate's own references from existing inner declarations.
#[derive(Default)]
struct BindingScopeNameIndex {
    names_by_binding: HashMap<BindingId, HashSet<Atom>>,
}

impl BindingScopeNameIndex {
    fn for_bindings(
        module: &Module,
        candidates: &HashSet<BindingId>,
        capture_sensitive_names: &HashSet<Atom>,
    ) -> Self {
        struct ScopeFrame {
            function_scope: bool,
            candidate_bindings: HashSet<BindingId>,
            declared_bindings: HashSet<BindingId>,
            references: HashSet<BindingId>,
        }

        impl ScopeFrame {
            fn new(function_scope: bool) -> Self {
                Self {
                    function_scope,
                    candidate_bindings: HashSet::default(),
                    declared_bindings: HashSet::default(),
                    references: HashSet::default(),
                }
            }
        }

        struct Builder<'a> {
            candidates: &'a HashSet<BindingId>,
            capture_sensitive_names: &'a HashSet<Atom>,
            scopes: Vec<ScopeFrame>,
            index: BindingScopeNameIndex,
        }

        impl Builder<'_> {
            fn push_scope(&mut self, function_scope: bool) {
                self.scopes.push(ScopeFrame::new(function_scope));
            }

            fn pop_scope(&mut self) {
                let Some(scope) = self.scopes.pop() else {
                    return;
                };
                let mut free_references = scope.references;
                free_references.retain(|reference| !scope.declared_bindings.contains(reference));
                for binding in scope.candidate_bindings {
                    self.index
                        .names_by_binding
                        .entry(binding)
                        .or_default()
                        .extend(free_references.iter().map(|reference| reference.0.clone()));
                }
                if let Some(parent) = self.scopes.last_mut() {
                    parent.references.extend(free_references);
                }
            }

            fn record_reference(&mut self, ident: &Ident) {
                if !self.capture_sensitive_names.contains(&ident.sym) {
                    return;
                }
                if let Some(scope) = self.scopes.last_mut() {
                    scope.references.insert((ident.sym.clone(), ident.ctxt));
                }
            }

            fn record_binding_at(&mut self, binding: &Ident, scope_idx: usize) {
                let id = (binding.sym.clone(), binding.ctxt);
                if self.capture_sensitive_names.contains(&binding.sym) {
                    self.scopes[scope_idx].declared_bindings.insert(id.clone());
                }
                if self.candidates.contains(&id) {
                    self.scopes[scope_idx].candidate_bindings.insert(id);
                }
            }

            fn record_binding_current(&mut self, binding: &Ident) {
                let Some(scope_idx) = self.scopes.len().checked_sub(1) else {
                    return;
                };
                self.record_binding_at(binding, scope_idx);
            }

            fn record_binding_function_scoped(&mut self, binding: &Ident) {
                let Some(scope_idx) = self.scopes.iter().rposition(|scope| scope.function_scope)
                else {
                    return;
                };
                self.record_binding_at(binding, scope_idx);
            }

            fn record_pat_current(&mut self, pat: &Pat) {
                self.record_pat_bindings(pat, false);
            }

            fn record_pat_function_scoped(&mut self, pat: &Pat) {
                self.record_pat_bindings(pat, true);
            }

            fn record_pat_bindings(&mut self, pat: &Pat, function_scoped: bool) {
                match pat {
                    Pat::Ident(binding) => {
                        if function_scoped {
                            self.record_binding_function_scoped(&binding.id);
                        } else {
                            self.record_binding_current(&binding.id);
                        }
                    }
                    Pat::Array(array) => {
                        for element in array.elems.iter().flatten() {
                            self.record_pat_bindings(element, function_scoped);
                        }
                    }
                    Pat::Object(object) => {
                        for property in &object.props {
                            match property {
                                ObjectPatProp::KeyValue(key_value) => {
                                    self.record_pat_bindings(&key_value.value, function_scoped)
                                }
                                ObjectPatProp::Assign(assign) => {
                                    if function_scoped {
                                        self.record_binding_function_scoped(&assign.key.id);
                                    } else {
                                        self.record_binding_current(&assign.key.id);
                                    }
                                }
                                ObjectPatProp::Rest(rest) => {
                                    self.record_pat_bindings(&rest.arg, function_scoped)
                                }
                            }
                        }
                    }
                    Pat::Assign(assign) => self.record_pat_bindings(&assign.left, function_scoped),
                    Pat::Rest(rest) => self.record_pat_bindings(&rest.arg, function_scoped),
                    Pat::Expr(_) | Pat::Invalid(_) => {}
                }
            }
        }

        impl Visit for Builder<'_> {
            fn visit_ident(&mut self, ident: &Ident) {
                self.record_reference(ident);
            }

            fn visit_function(&mut self, function: &Function) {
                self.push_scope(true);
                for param in &function.params {
                    self.record_pat_current(&param.pat);
                }
                function.visit_children_with(self);
                self.pop_scope();
            }

            fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
                self.push_scope(true);
                for param in &arrow.params {
                    self.record_pat_current(param);
                }
                arrow.visit_children_with(self);
                self.pop_scope();
            }

            // Getter/setter props need no overrides: the computed key is a
            // direct child visited in the enclosing scope, and the backing
            // `Function` child routes through `visit_function`, which pushes
            // the accessor's function scope and records its params.

            fn visit_constructor(&mut self, constructor: &swc_core::ecma::ast::Constructor) {
                constructor.key.visit_with(self);
                self.push_scope(true);
                for param in &constructor.params {
                    match param {
                        swc_core::ecma::ast::ParamOrTsParamProp::Param(param) => {
                            self.record_pat_current(&param.pat)
                        }
                        swc_core::ecma::ast::ParamOrTsParamProp::TsParamProp(param) => {
                            match &param.param {
                                swc_core::ecma::ast::TsParamPropParam::Ident(binding) => {
                                    self.record_binding_current(&binding.id)
                                }
                                swc_core::ecma::ast::TsParamPropParam::Assign(assign) => {
                                    self.record_pat_current(&assign.left)
                                }
                            }
                        }
                    }
                }
                constructor.params.visit_with(self);
                if let Some(body) = &constructor.body {
                    body.visit_with(self);
                }
                self.pop_scope();
            }

            fn visit_block_stmt(&mut self, block: &swc_core::ecma::ast::BlockStmt) {
                self.push_scope(false);
                block.visit_children_with(self);
                self.pop_scope();
            }

            // Keep a body frame separate from the param frame pushed by
            // `visit_function`, mirroring the pre-swc-29 shape where function
            // bodies were blocks: a param referenced in the body stays out of
            // the body frame's declared bindings and remains forbidden for
            // body-level candidates. The rename checker independently rejects
            // captures, so this only keeps first-choice proposals collision
            // free instead of relying on that fallback.
            fn visit_function_body(&mut self, body: &swc_core::ecma::ast::FunctionBody) {
                self.push_scope(false);
                body.visit_children_with(self);
                self.pop_scope();
            }

            fn visit_catch_clause(&mut self, catch: &swc_core::ecma::ast::CatchClause) {
                self.push_scope(false);
                if let Some(param) = &catch.param {
                    self.record_pat_current(param);
                }
                catch.visit_children_with(self);
                self.pop_scope();
            }

            fn visit_var_decl(&mut self, declaration: &VarDecl) {
                for declarator in &declaration.decls {
                    if declaration.kind == VarDeclKind::Var {
                        self.record_pat_function_scoped(&declarator.name);
                    } else {
                        self.record_pat_current(&declarator.name);
                    }
                }
                declaration.visit_children_with(self);
            }

            fn visit_fn_decl(&mut self, declaration: &FnDecl) {
                self.record_binding_current(&declaration.ident);
                declaration.visit_children_with(self);
            }

            fn visit_class_decl(&mut self, declaration: &ClassDecl) {
                self.record_binding_current(&declaration.ident);
                declaration.visit_children_with(self);
            }

            fn visit_fn_expr(&mut self, expression: &FnExpr) {
                if let Some(ident) = &expression.ident {
                    self.push_scope(false);
                    self.record_binding_current(ident);
                    ident.visit_with(self);
                    expression.function.visit_with(self);
                    self.pop_scope();
                } else {
                    expression.function.visit_with(self);
                }
            }

            fn visit_class_expr(&mut self, expression: &ClassExpr) {
                if let Some(ident) = &expression.ident {
                    self.push_scope(false);
                    self.record_binding_current(ident);
                    ident.visit_with(self);
                    expression.class.visit_with(self);
                    self.pop_scope();
                } else {
                    expression.class.visit_with(self);
                }
            }

            fn visit_import_decl(&mut self, declaration: &ImportDecl) {
                // Imported names are module API labels, not local references;
                // only local specifier bindings participate in capture.
                for specifier in &declaration.specifiers {
                    let local = match specifier {
                        ImportSpecifier::Default(default) => &default.local,
                        ImportSpecifier::Named(named) => &named.local,
                        ImportSpecifier::Namespace(namespace) => &namespace.local,
                    };
                    self.record_binding_current(local);
                    local.visit_with(self);
                }
            }
        }

        if candidates.is_empty() {
            return Self::default();
        }

        let mut builder = Builder {
            candidates,
            capture_sensitive_names,
            scopes: vec![ScopeFrame::new(true)],
            index: Self::default(),
        };
        module.visit_children_with(&mut builder);
        builder.pop_scope();
        builder.index
    }

    fn rename_would_capture(&self, binding: &BindingId, new_name: &Atom) -> bool {
        self.names_by_binding
            .get(binding)
            .is_some_and(|names| names.contains(new_name))
    }
}

/// Declaration facts that decide whether a value-position key may name a
/// binding that also has other uses.
#[derive(Default)]
struct BindingTraits {
    destructured: bool,
    class: bool,
    /// Property names tested by a boolean-valued initializer.
    boolean_tested_props: Vec<Atom>,
}

impl BindingTraits {
    fn allows_target_with_other_uses(&self, target: &str) -> bool {
        !matches!(target, "type" | "name" | "value" | "data" | "key")
            && !target.starts_with('$')
            && !self.destructured
            && !self.class
            && !self
                .boolean_tested_props
                .iter()
                .any(|p| p.as_ref() == target)
    }
}

#[derive(Default)]
struct BindingCollector {
    short_bindings: HashMap<BindingId, BindingTraits>,
    /// Depth of enclosing object/array patterns, reset at function and
    /// class boundaries so a nested function's parameters are not counted.
    pattern_depth: usize,
}

impl BindingCollector {
    fn record(&mut self, id: &Ident) -> Option<&mut BindingTraits> {
        if !is_likely_generated_alias(&id.sym) {
            return None;
        }
        let destructured = self.pattern_depth > 0;
        let traits = self
            .short_bindings
            .entry((id.sym.clone(), id.ctxt))
            .or_default();
        traits.destructured |= destructured;
        Some(traits)
    }

    fn with_pattern_depth_reset(&mut self, visit: impl FnOnce(&mut Self)) {
        let depth = std::mem::take(&mut self.pattern_depth);
        visit(self);
        self.pattern_depth = depth;
    }
}

/// When `expr` is boolean-valued (`!x`, a comparison, or `&&`/`||` over
/// boolean operands), the property names it tests: `!!s.icon` → `icon`,
/// `s.weight > 0` → `weight`, `t && !!s.badge` → `badge`.
fn boolean_tested_props(expr: &Expr) -> Option<Vec<Atom>> {
    fn operand_prop(expr: &Expr) -> Option<Atom> {
        match expr.unwrap_parens() {
            Expr::Member(member) => static_member_prop_name(&member.prop).map(Atom::from),
            Expr::OptChain(chain) => match &*chain.base {
                swc_core::ecma::ast::OptChainBase::Member(member) => {
                    static_member_prop_name(&member.prop).map(Atom::from)
                }
                _ => None,
            },
            _ => None,
        }
    }
    fn operand(expr: &Expr) -> Vec<Atom> {
        boolean_tested_props(expr).unwrap_or_else(|| operand_prop(expr).into_iter().collect())
    }
    use swc_core::ecma::ast::{BinaryOp, UnaryOp};
    match expr.unwrap_parens() {
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => Some(operand(&unary.arg)),
        Expr::Bin(bin) => match bin.op {
            BinaryOp::EqEq
            | BinaryOp::NotEq
            | BinaryOp::EqEqEq
            | BinaryOp::NotEqEq
            | BinaryOp::Lt
            | BinaryOp::LtEq
            | BinaryOp::Gt
            | BinaryOp::GtEq
            | BinaryOp::InstanceOf
            | BinaryOp::In => {
                let mut props = operand(&bin.left);
                props.extend(operand(&bin.right));
                Some(props)
            }
            BinaryOp::LogicalAnd => boolean_tested_props(&bin.right).map(|mut props| {
                props.extend(boolean_tested_props(&bin.left).unwrap_or_default());
                props
            }),
            BinaryOp::LogicalOr => {
                let mut props = boolean_tested_props(&bin.left)?;
                props.extend(boolean_tested_props(&bin.right)?);
                Some(props)
            }
            _ => None,
        },
        _ => None,
    }
}

impl Visit for BindingCollector {
    fn visit_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(bi) => {
                self.record(&bi.id);
            }
            Pat::Object(_) | Pat::Array(_) => {
                self.pattern_depth += 1;
                pat.visit_children_with(self);
                self.pattern_depth -= 1;
                return;
            }
            _ => {}
        }
        pat.visit_children_with(self);
    }

    fn visit_object_pat_prop(&mut self, prop: &ObjectPatProp) {
        if let ObjectPatProp::Assign(a) = prop {
            self.record(&a.key.id);
        }
        prop.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, decl: &swc_core::ecma::ast::VarDeclarator) {
        decl.visit_children_with(self);
        let (Pat::Ident(bi), Some(init)) = (&decl.name, &decl.init) else {
            return;
        };
        let is_class = matches!(init.unwrap_parens(), Expr::Class(_));
        let tested = boolean_tested_props(init);
        if let Some(traits) = self.record(&bi.id) {
            traits.class |= is_class;
            if let Some(props) = tested {
                traits.boolean_tested_props.extend(props);
            }
        }
    }

    fn visit_function(&mut self, function: &Function) {
        self.with_pattern_depth_reset(|this| function.visit_children_with(this));
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.with_pattern_depth_reset(|this| arrow.visit_children_with(this));
    }

    fn visit_class(&mut self, class: &Class) {
        self.with_pattern_depth_reset(|this| class.visit_children_with(this));
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.record(&decl.ident);
        decl.function.visit_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        if let Some(traits) = self.record(&decl.ident) {
            traits.class = true;
        }
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
            if let Some(traits) = self.record(ident) {
                traits.class = true;
            }
        }
        ce.class.visit_with(self);
    }

    fn visit_import_decl(&mut self, decl: &ImportDecl) {
        for spec in &decl.specifiers {
            match spec {
                ImportSpecifier::Default(d) => self.record(&d.local),
                ImportSpecifier::Named(n) => self.record(&n.local),
                ImportSpecifier::Namespace(ns) => self.record(&ns.local),
            };
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
    traits: BindingTraits,
}

impl ClassificationState {
    fn single_target(&self) -> Option<String> {
        if self.value_targets.len() != 1 {
            return None;
        }
        let target = self.value_targets.keys().next()?;
        if self.other_uses > 0 && !self.traits.allows_target_with_other_uses(target) {
            return None;
        }
        Some(target.clone())
    }
}

struct ValuePositionClassifier {
    states: HashMap<BindingId, ClassificationState>,
}

impl ValuePositionClassifier {
    fn new(bindings: HashMap<BindingId, BindingTraits>) -> Self {
        let states = bindings
            .into_iter()
            .map(|(k, traits)| {
                (
                    k,
                    ClassificationState {
                        traits,
                        ..Default::default()
                    },
                )
            })
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
        // Treat `get name() { return x; }` as a value-position hint:
        // the getter name suggests what `x` should be called.
        if let Prop::Getter(getter) = prop {
            if let Some(id) = getter_single_return_ident(getter) {
                let bid = (id.sym.clone(), id.ctxt);
                if self.states.contains_key(&bid) {
                    match key_as_ident_target(&getter.key) {
                        Some(name) => self.record_value_use(&bid, name),
                        None => self.record_other_use(&bid),
                    }
                    return;
                }
            }
            if let Some(body) = &getter.function.body {
                body.visit_with(self);
            }
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

fn getter_single_return_ident(getter: &GetterProp) -> Option<&Ident> {
    let body = getter.function.body.as_ref()?;
    if body.stmts.len() != 1 {
        return None;
    }
    let Stmt::Return(ret) = &body.stmts[0] else {
        return None;
    };
    let Expr::Ident(id) = ret.arg.as_deref()? else {
        return None;
    };
    Some(id)
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
// Call-site parameter renames
//
// A short parameter of a function whose every reference is a direct call is
// renamed after the argument all those calls pass at its position:
//
//   function f(e) { return e.trim(); }  f(input.text); f(text);
//                                        → function f(text) { ... }
//
// Requirements, each measured against original names:
//   - The function binding is declared once, never written, not exported,
//     and never used except as a direct callee (a function that escapes has
//     callers we cannot see).
//   - Every call passes a named argument (`x` or `obj.x`) at the position,
//     and all names agree. A spread at or before the position, a missing
//     argument, or any other expression disqualifies.
//   - The name carries meaning: a Wakaru `_N` suffix is dropped; short,
//     mangled-looking (`kQz`, `J99`), reserved-property, `t_x`-synthesized,
//     and React ref `current` names are rejected.
//   - The parameter is used, never written (minifiers reuse parameters as
//     scratch variables), and not already backed by a shorthand property;
//     the name does not already occur anywhere inside the function.
//
// More callers do not make the name more reliable (widely called helpers take
// a general parameter while callers pass something specific), and generic
// names such as `options` or `type` are usually right here, unlike in value
// position. Runs after value-position renames, which are more precise.
// ============================================================

/// `value_named` holds bindings value-position renames just named; their
/// new name can still look short (`fn`) but is the better evidence.
fn call_site_param_rename_module(module: &mut Module, value_named: &HashSet<BindingId>) {
    if has_dynamic_scope_construct(module) {
        return;
    }
    let mut functions = CallSiteFunctionCollector::default();
    module.visit_with(&mut functions);
    if functions.candidates.iter().all(|c| c.binding.is_none()) {
        return;
    }

    let by_binding: HashMap<BindingId, usize> = functions
        .candidates
        .iter()
        .enumerate()
        .filter_map(|(idx, c)| c.binding.clone().map(|bid| (bid, idx)))
        .collect();
    let mut uses = CallSiteUseCollector {
        functions: &by_binding,
        uses: HashMap::default(),
    };
    module.visit_with(&mut uses);
    let exported_bindings = collect_exported_binding_ids(module);

    let mut proposals: Vec<(Atom, usize, usize, BindingId)> = Vec::new();
    for (bid, &idx) in &by_binding {
        let Some(use_info) = uses.uses.get(bid) else {
            continue;
        };
        if use_info.declarations != 1
            || use_info.escapes > 0
            || use_info.calls.is_empty()
            || exported_bindings.contains(bid)
        {
            continue;
        }
        let candidate = &functions.candidates[idx];
        for (position, param) in candidate.params.iter().enumerate() {
            let Some(param) = param else {
                continue;
            };
            if value_named.contains(param)
                || functions.keyed_params.contains(param)
                || functions.param_writes.get(param).copied().unwrap_or(0) > 1
                || functions.param_refs.get(param).copied().unwrap_or(0) == 0
            {
                continue;
            }
            let Some(name) = unanimous_call_site_name(&use_info.calls, position) else {
                continue;
            };
            if candidate.names.contains(&name) || name == param.0 {
                continue;
            }
            proposals.push((name, idx, position, param.clone()));
        }
    }
    proposals.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

    // Proposals are checked against the names each function already holds;
    // two proposals of the same name conflict when their functions are the
    // same or nested, since the inner parameter could capture the outer one.
    let nested = |a: usize, b: usize| {
        a == b
            || functions.candidates[a].ancestors.contains(&b)
            || functions.candidates[b].ancestors.contains(&a)
    };
    let mut accepted: HashMap<Atom, Vec<usize>> = HashMap::default();
    let mut renames = Vec::new();
    for (name, idx, _, param) in proposals {
        let taken = accepted.entry(name.clone()).or_default();
        if taken.iter().any(|&other| nested(idx, other)) {
            continue;
        }
        taken.push(idx);
        renames.push(BindingRename {
            old: param,
            new: name,
        });
    }
    rename_bindings_in_module(module, &renames);
}

/// Property names that name a mechanism rather than a value.
const CALL_SITE_RESERVED_NAMES: &[&str] = &[
    "default",
    "length",
    "prototype",
    "constructor",
    "then",
    "call",
    "apply",
    "bind",
    "value",
    "exports",
    "module",
    "require",
    "arguments",
    "undefined",
    "eval",
    // React ref containers: `xRef.current` says nothing about the value.
    "current",
];

/// A name that reads like a real identifier, with any Wakaru `_N`
/// disambiguation suffix dropped.
fn call_site_name(raw: &str) -> Option<Atom> {
    if is_synthesized_prefixed_name(raw) {
        return None;
    }
    let base = match raw.rsplit_once('_') {
        Some((base, digits))
            if !base.is_empty()
                && !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit()) =>
        {
            base
        }
        _ => raw,
    };
    if is_likely_generated_alias(base)
        || looks_mangled(base)
        || CALL_SITE_RESERVED_NAMES.contains(&base)
        || is_reserved_binding_name(base)
        || !is_valid_js_ident(base)
    {
        return None;
    }
    Some(Atom::from(base))
}

/// Short names a minifier emits that `is_likely_generated_alias` does not
/// cover: one letter plus digits (`J99`) or three letters in mixed case
/// (`kQz`).
fn looks_mangled(name: &str) -> bool {
    name.chars().count() <= 3
        && (name.chars().any(|c| c.is_ascii_digit())
            || name.chars().skip(1).any(|c| c.is_ascii_uppercase()))
}

/// Wakaru keeps a minified stem when it builds a name from a property:
/// `t_nextValue`, `ab_value`.
fn is_synthesized_prefixed_name(name: &str) -> bool {
    let chars: Vec<char> = name.chars().collect();
    let stem_char = |c: char| c.is_ascii_alphanumeric() || c == '$';
    let underscore_at = if chars.len() > 2 && chars[1] == '_' {
        1
    } else if chars.len() > 3 && chars[2] == '_' && stem_char(chars[1]) {
        2
    } else {
        return false;
    };
    (chars[0].is_ascii_alphabetic() || chars[0] == '$')
        && chars[underscore_at + 1].is_ascii_alphabetic()
}

fn unanimous_call_site_name(calls: &[Vec<Option<Atom>>], position: usize) -> Option<Atom> {
    let mut agreed: Option<&Atom> = None;
    for call in calls {
        let name = call.get(position)?.as_ref()?;
        match agreed {
            Some(previous) if previous != name => return None,
            _ => agreed = Some(name),
        }
    }
    agreed.cloned()
}

struct CallSiteFunction {
    /// The constant binding that names the function, when it has one.
    binding: Option<BindingId>,
    /// Short plain-identifier parameters by position.
    params: Vec<Option<BindingId>>,
    /// Indices of enclosing named functions.
    ancestors: Vec<usize>,
    /// Every identifier spelled inside the function, parameters included.
    names: HashSet<Atom>,
}

/// Finds functions declared as `function f() {}` or `const f = ... =>` /
/// `function () {}`, their short parameters, the names spelled inside them,
/// and how often each such parameter is referenced.
#[derive(Default)]
struct CallSiteFunctionCollector {
    candidates: Vec<CallSiteFunction>,
    stack: Vec<usize>,
    pending_binding: Option<BindingId>,
    param_refs: HashMap<BindingId, usize>,
    /// Parameters used as a shorthand property (`{ fn }`): the key already
    /// backs the current name.
    keyed_params: HashSet<BindingId>,
    /// Binding sites per parameter: the parameter itself plus every write.
    /// Minifiers reuse parameters as scratch variables, so a written
    /// parameter can hold values unrelated to what the callers pass.
    param_writes: HashMap<BindingId, usize>,
}

impl CallSiteFunctionCollector {
    fn record_name(&mut self, sym: &Atom) {
        for &idx in &self.stack {
            self.candidates[idx].names.insert(sym.clone());
        }
    }

    fn enter<'a>(&mut self, params: impl Iterator<Item = &'a Pat>) -> Option<usize> {
        let binding = self.pending_binding.take()?;
        let params: Vec<Option<BindingId>> = params
            .map(|pat| match pat {
                Pat::Ident(b) if is_likely_generated_alias(&b.id.sym) => {
                    Some((b.id.sym.clone(), b.id.ctxt))
                }
                _ => None,
            })
            .collect();
        for param in params.iter().flatten() {
            self.param_refs.entry(param.clone()).or_default();
        }
        let idx = self.candidates.len();
        self.candidates.push(CallSiteFunction {
            binding: Some(binding),
            params,
            ancestors: self.stack.clone(),
            names: HashSet::default(),
        });
        self.stack.push(idx);
        Some(idx)
    }

    fn leave(&mut self, entered: Option<usize>) {
        if entered.is_some() {
            self.stack.pop();
        }
    }
}

impl Visit for CallSiteFunctionCollector {
    fn visit_ident(&mut self, id: &Ident) {
        self.record_name(&id.sym);
        if let Some(count) = self.param_refs.get_mut(&(id.sym.clone(), id.ctxt)) {
            *count += 1;
        }
    }

    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        self.record_name(&binding.id.sym);
        let bid = (binding.id.sym.clone(), binding.id.ctxt);
        if self.param_refs.contains_key(&bid) {
            *self.param_writes.entry(bid).or_default() += 1;
        }
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        if let Expr::Ident(id) = update.arg.unwrap_parens() {
            let bid = (id.sym.clone(), id.ctxt);
            if self.param_refs.contains_key(&bid) {
                *self.param_writes.entry(bid).or_default() += 1;
            }
        }
        update.visit_children_with(self);
    }

    fn visit_prop(&mut self, prop: &Prop) {
        if let Prop::Shorthand(id) = prop {
            let bid = (id.sym.clone(), id.ctxt);
            if self.param_refs.contains_key(&bid) {
                self.keyed_params.insert(bid);
            }
        }
        prop.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.record_name(&decl.ident.sym);
        self.pending_binding = Some((decl.ident.sym.clone(), decl.ident.ctxt));
        decl.function.visit_with(self);
    }

    fn visit_fn_expr(&mut self, fn_expr: &FnExpr) {
        if let Some(ident) = &fn_expr.ident {
            self.record_name(&ident.sym);
        }
        fn_expr.function.visit_with(self);
    }

    fn visit_var_declarator(&mut self, decl: &swc_core::ecma::ast::VarDeclarator) {
        decl.name.visit_with(self);
        let Some(init) = &decl.init else {
            return;
        };
        if let Pat::Ident(b) = &decl.name {
            if matches!(init.unwrap_parens(), Expr::Fn(_) | Expr::Arrow(_)) {
                self.pending_binding = Some((b.id.sym.clone(), b.id.ctxt));
            }
        }
        init.visit_with(self);
        self.pending_binding = None;
    }

    fn visit_function(&mut self, function: &Function) {
        let entered = self.enter(function.params.iter().map(|p| &p.pat));
        function.visit_children_with(self);
        self.leave(entered);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        let entered = self.enter(arrow.params.iter());
        arrow.visit_children_with(self);
        self.leave(entered);
    }

    fn visit_constructor(&mut self, ctor: &Constructor) {
        self.pending_binding = None;
        ctor.visit_children_with(self);
    }
}

#[derive(Default)]
struct CallSiteUses {
    /// Binding sites: the declaration plus any redeclaration or write.
    declarations: usize,
    /// References other than as a direct callee.
    escapes: usize,
    /// Per call, the usable name passed at each argument position.
    calls: Vec<Vec<Option<Atom>>>,
}

struct CallSiteUseCollector<'a> {
    functions: &'a HashMap<BindingId, usize>,
    uses: HashMap<BindingId, CallSiteUses>,
}

impl CallSiteUseCollector<'_> {
    fn entry(&mut self, id: &Ident) -> Option<&mut CallSiteUses> {
        let bid = (id.sym.clone(), id.ctxt);
        if !self.functions.contains_key(&bid) {
            return None;
        }
        Some(self.uses.entry(bid).or_default())
    }
}

impl Visit for CallSiteUseCollector<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        if let Some(uses) = self.entry(id) {
            uses.escapes += 1;
        }
    }

    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        if let Some(uses) = self.entry(&binding.id) {
            uses.declarations += 1;
        }
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        if let Some(uses) = self.entry(&decl.ident) {
            uses.declarations += 1;
        }
        decl.function.visit_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Ident(id) = callee.as_ref() {
                if self.functions.contains_key(&(id.sym.clone(), id.ctxt)) {
                    let mut hidden = false;
                    let names = call
                        .args
                        .iter()
                        .map(|arg| {
                            // A spread hides this and every later position.
                            hidden |= arg.spread.is_some();
                            if hidden {
                                return None;
                            }
                            let raw = match arg.expr.unwrap_parens() {
                                Expr::Ident(arg_id) => arg_id.sym.clone(),
                                Expr::Member(MemberExpr {
                                    prop: MemberProp::Ident(prop),
                                    ..
                                }) => prop.sym.clone(),
                                _ => return None,
                            };
                            call_site_name(&raw)
                        })
                        .collect();
                    if let Some(uses) = self.entry(id) {
                        uses.calls.push(names);
                    }
                    call.args.visit_with(self);
                    return;
                }
            }
        }
        call.visit_children_with(self);
    }
}

// ============================================================
// Structural-role renames
//
// Some bindings have a role the language or a standard method fixes,
// whatever the code around them says:
//
//   try { ... } catch (e) { report(e); }   → catch (error)
//   new Promise((e, t) => ...)             → (resolve, reject) => ...
//   list.reduce((e, t, n) => ..., init)    → (acc, item, index) => ...
//   for (...; t < rows.length; ...) { const n = rows[t]; }
//                                          → const row = rows[t];
//
// A reduce element is named `key` over `Object.keys(...)` (`entry` over
// `Object.entries(...)`, also through `.sort()` and similar), the singular of
// the receiver's name when the plural is unambiguous (`orders` → `order`),
// and `item` otherwise. A loop element takes the singular of the array it
// indexes with the loop counter; without an unambiguous singular it keeps
// its name. Loop counters are left alone: single letters collide with
// whatever else the minifier spelled, and a letter for a letter adds little.
//
// Only short bindings are renamed, and only when the new name does not
// already occur inside the scope (catch clause, callback, or loop). A catch
// parameter must be read. Catch, executor, reduce element and index, and
// loop element bindings must not be written (minifiers reuse bindings as
// scratch variables); an accumulator may be, that is its role. Executor
// and reduce parameters are renamed even when unused, since the name
// documents the position. `Promise` and `Object` must be the globals. A
// nested scope keeps its short name when it reads an outer binding that
// takes the same name, so the rename cannot capture it.
// ============================================================

fn role_rename_module(module: &mut Module, unresolved_mark: Mark) {
    if has_dynamic_scope_construct(module) {
        return;
    }
    let mut collector = RoleCollector {
        unresolved_mark,
        scopes: Vec::new(),
        stack: Vec::new(),
        tracked: HashMap::default(),
    };
    module.visit_with(&mut collector);

    let mut accepted: HashMap<Atom, Vec<(usize, Atom)>> = HashMap::default();
    let mut renames = Vec::new();
    for (idx, scope) in collector.scopes.iter().enumerate() {
        for param in &scope.params {
            let uses = collector
                .tracked
                .get(&param.binding)
                .copied()
                .unwrap_or_default();
            if (param.forbid_writes && uses.writes > 1) || (param.require_read && uses.refs == 0) {
                continue;
            }
            if scope.names.contains(&param.target) {
                continue;
            }
            // An inner scope that reads the outer binding would capture it
            // once both take the same name.
            let taken = accepted.entry(param.target.clone()).or_default();
            let captures = taken.iter().any(|(other, old)| {
                let other_scope = &collector.scopes[*other];
                (scope.ancestors.contains(other) && scope.names.contains(old))
                    || (other_scope.ancestors.contains(&idx)
                        && other_scope.names.contains(&param.binding.0))
            });
            if captures {
                continue;
            }
            taken.push((idx, param.binding.0.clone()));
            renames.push(BindingRename {
                old: param.binding.clone(),
                new: param.target.clone(),
            });
        }
    }
    rename_bindings_in_module(module, &renames);
}

struct RoleParam {
    binding: BindingId,
    target: Atom,
    require_read: bool,
    forbid_writes: bool,
}

impl RoleParam {
    fn new(binding: BindingId, target: &str) -> Self {
        Self {
            binding,
            target: Atom::from(target),
            require_read: false,
            forbid_writes: true,
        }
    }
}

struct RoleScope {
    params: Vec<RoleParam>,
    /// Indices of enclosing role scopes.
    ancestors: Vec<usize>,
    /// Every identifier spelled inside the scope, parameters included.
    names: HashSet<Atom>,
}

#[derive(Clone, Copy, Default)]
struct RoleParamUses {
    refs: usize,
    /// Binding sites: the declaration plus every write.
    writes: usize,
}

struct RoleCollector {
    unresolved_mark: Mark,
    scopes: Vec<RoleScope>,
    stack: Vec<usize>,
    tracked: HashMap<BindingId, RoleParamUses>,
}

impl RoleCollector {
    fn record_name(&mut self, sym: &Atom) {
        for &idx in &self.stack {
            self.scopes[idx].names.insert(sym.clone());
        }
    }

    /// Visits `node` inside a new role scope when there are parameters to
    /// name, and plainly otherwise.
    fn with_scope(&mut self, params: Vec<RoleParam>, visit: impl FnOnce(&mut Self)) {
        if params.is_empty() {
            visit(self);
            return;
        }
        for param in &params {
            self.tracked.entry(param.binding.clone()).or_default();
        }
        let idx = self.scopes.len();
        self.scopes.push(RoleScope {
            params,
            ancestors: self.stack.clone(),
            names: HashSet::default(),
        });
        self.stack.push(idx);
        visit(self);
        self.stack.pop();
    }

    fn short_param(pat: &Pat) -> Option<BindingId> {
        match pat {
            Pat::Ident(b) if is_likely_generated_alias(&b.id.sym) => {
                Some((b.id.sym.clone(), b.id.ctxt))
            }
            _ => None,
        }
    }

    fn callback_params(expr: &Expr) -> Option<Vec<&Pat>> {
        match expr.unwrap_parens() {
            Expr::Arrow(arrow) => Some(arrow.params.iter().collect()),
            Expr::Fn(f) => Some(f.function.params.iter().map(|p| &p.pat).collect()),
            _ => None,
        }
    }

    /// The element name an `Object` enumeration gives its array:
    /// `Object.keys(x)` / `getOwnPropertyNames` → `key`, `Object.entries(x)`
    /// → `entry`, with the global `Object`. Looks through calls that keep
    /// the elements (`Object.keys(x).sort()`).
    fn object_enumeration_element(&self, expr: &Expr) -> Option<&'static str> {
        let Expr::Call(call) = expr.unwrap_parens() else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(prop),
            ..
        }) = callee.as_ref()
        else {
            return None;
        };
        if matches!(obj.as_ref(), Expr::Ident(id) if is_unresolved_ident(id, "Object", self.unresolved_mark))
        {
            return match prop.sym.as_ref() {
                "keys" | "getOwnPropertyNames" => Some("key"),
                "entries" => Some("entry"),
                _ => None,
            };
        }
        match prop.sym.as_ref() {
            "sort" | "filter" | "slice" | "reverse" => self.object_enumeration_element(obj),
            _ => None,
        }
    }

    fn reduce_element_name(&self, receiver: &Expr) -> String {
        if let Some(name) = self.object_enumeration_element(receiver) {
            return name.to_string();
        }
        role_tail_name(receiver)
            .and_then(|name| singular_name(&name))
            .unwrap_or_else(|| "item".to_string())
    }

    /// Loop elements of `for (let t = 0; t < ARR.length; t++) { const n =
    /// ARR[t]; }`, also through a cached `r = ARR.length` in the init.
    fn loop_element_params(for_stmt: &ForStmt) -> Vec<RoleParam> {
        let Some(VarDeclOrExpr::VarDecl(init)) = &for_stmt.init else {
            return Vec::new();
        };
        let Some(Pat::Ident(counter)) = init.decls.first().map(|d| &d.name) else {
            return Vec::new();
        };
        let counter_id = (counter.id.sym.clone(), counter.id.ctxt);
        let is_counter = |e: &Expr| matches!(e.unwrap_parens(), Expr::Ident(id) if (id.sym.clone(), id.ctxt) == counter_id);
        let Some(Expr::Bin(test)) = for_stmt.test.as_deref() else {
            return Vec::new();
        };
        use swc_core::ecma::ast::BinaryOp;
        if !matches!(
            test.op,
            BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq
        ) {
            return Vec::new();
        }
        let bound = if is_counter(&test.left) {
            test.right.as_ref()
        } else if is_counter(&test.right) {
            test.left.as_ref()
        } else {
            return Vec::new();
        };
        let length_of = |e: &Expr| match e.unwrap_parens() {
            Expr::Member(MemberExpr {
                obj,
                prop: MemberProp::Ident(prop),
                ..
            }) if prop.sym == "length" => Some(obj.as_ref().clone()),
            _ => None,
        };
        let array = length_of(bound).or_else(|| {
            let Expr::Ident(bound_id) = bound.unwrap_parens() else {
                return None;
            };
            init.decls
                .iter()
                .find_map(|d| match (&d.name, d.init.as_deref()) {
                    (Pat::Ident(b), Some(value))
                        if b.id.sym == bound_id.sym && b.id.ctxt == bound_id.ctxt =>
                    {
                        length_of(value)
                    }
                    _ => None,
                })
        });
        let Some(array) = array else {
            return Vec::new();
        };
        if !is_plain_access_path(&array) {
            return Vec::new();
        }
        let Some(target) = role_tail_name(&array)
            .filter(|name| name != "arguments")
            .and_then(|name| singular_name(&name))
        else {
            return Vec::new();
        };
        let Stmt::Block(body) = for_stmt.body.as_ref() else {
            return Vec::new();
        };
        let mut params = Vec::new();
        for stmt in &body.stmts {
            let Stmt::Decl(Decl::Var(var)) = stmt else {
                continue;
            };
            for decl in &var.decls {
                let (Some(binding), Some(Expr::Member(member))) = (
                    Self::short_param(&decl.name),
                    decl.init.as_deref().map(Expr::unwrap_parens),
                ) else {
                    continue;
                };
                let MemberProp::Computed(index) = &member.prop else {
                    continue;
                };
                if is_counter(&index.expr) && same_access_path(&member.obj, &array) {
                    let mut param = RoleParam::new(binding, &target);
                    param.require_read = true;
                    params.push(param);
                }
            }
        }
        params
    }
}

/// `a`, `a.b`, `a.b.c` — reads with no side effects to compare by shape.
fn is_plain_access_path(expr: &Expr) -> bool {
    match expr.unwrap_parens() {
        Expr::Ident(_) | Expr::This(_) => true,
        Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(_),
            ..
        }) => is_plain_access_path(obj),
        _ => false,
    }
}

fn same_access_path(a: &Expr, b: &Expr) -> bool {
    match (a.unwrap_parens(), b.unwrap_parens()) {
        (Expr::Ident(x), Expr::Ident(y)) => x.sym == y.sym && x.ctxt == y.ctxt,
        (Expr::This(_), Expr::This(_)) => true,
        (
            Expr::Member(MemberExpr {
                obj: xo,
                prop: MemberProp::Ident(xp),
                ..
            }),
            Expr::Member(MemberExpr {
                obj: yo,
                prop: MemberProp::Ident(yp),
                ..
            }),
        ) => xp.sym == yp.sym && same_access_path(xo, yo),
        _ => false,
    }
}

/// The name an access path ends in (`e.rows` → `rows`), without the
/// minified stem Wakaru keeps on names it builds (`e_rows` → `rows`).
fn role_tail_name(expr: &Expr) -> Option<String> {
    let raw = match expr.unwrap_parens() {
        Expr::Ident(id) => id.sym.to_string(),
        Expr::Member(MemberExpr {
            prop: MemberProp::Ident(prop),
            ..
        }) => prop.sym.to_string(),
        _ => return None,
    };
    let name = if is_synthesized_prefixed_name(&raw) {
        raw.split_once('_').map(|(_, rest)| rest.to_string())?
    } else {
        raw
    };
    Some(name)
}

/// Singular of a plural identifier, only when the plural is unambiguous.
/// camelCase names change their last word (`nodeIndices` → `nodeIndex`).
fn singular_name(name: &str) -> Option<String> {
    let split = name
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_ascii_uppercase())
        .map_or(0, |(i, _)| i);
    let (head, word) = name.split_at(split);
    let lower = word.to_ascii_lowercase();
    let singular = match lower.as_str() {
        "children" => "child".to_string(),
        "people" => "person".to_string(),
        "indices" => "index".to_string(),
        "vertices" => "vertex".to_string(),
        "matrices" => "matrix".to_string(),
        "caches" => "cache".to_string(),
        "leaves" => "leaf".to_string(),
        "halves" => "half".to_string(),
        "lives" => "life".to_string(),
        "aliases" => "alias".to_string(),
        "movies" => "movie".to_string(),
        "cookies" => "cookie".to_string(),
        // Ambiguous or not a plural.
        "axes" | "series" | "species" | "news" | "analyses" => return None,
        w if w.ends_with("uses") || w.ends_with("oes") => return None,
        w if w.ends_with("ies") && w.len() > 4 => format!("{}y", &w[..w.len() - 3]),
        w if ["ches", "shes", "xes", "sses", "zzes"]
            .iter()
            .any(|suffix| w.ends_with(suffix)) =>
        {
            w[..w.len() - 2].to_string()
        }
        w if w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") && !w.ends_with("is") => {
            w[..w.len() - 1].to_string()
        }
        _ => return None,
    };
    // Restore the word's leading capital (`SheetNames` → `SheetName`).
    let singular = if word.starts_with(|c: char| c.is_ascii_uppercase()) {
        let mut chars = singular.chars();
        chars
            .next()
            .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())?
    } else {
        singular
    };
    let result = format!("{head}{singular}");
    (!is_likely_generated_alias(&result)
        && !looks_mangled(&result)
        && is_valid_js_ident(&result)
        && !is_reserved_binding_name(&result))
    .then_some(result)
}

impl Visit for RoleCollector {
    fn visit_ident(&mut self, id: &Ident) {
        self.record_name(&id.sym);
        if let Some(uses) = self.tracked.get_mut(&(id.sym.clone(), id.ctxt)) {
            uses.refs += 1;
        }
    }

    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        self.record_name(&binding.id.sym);
        if let Some(uses) = self
            .tracked
            .get_mut(&(binding.id.sym.clone(), binding.id.ctxt))
        {
            uses.writes += 1;
        }
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        if let Expr::Ident(id) = update.arg.unwrap_parens() {
            if let Some(uses) = self.tracked.get_mut(&(id.sym.clone(), id.ctxt)) {
                uses.writes += 1;
            }
        }
        update.visit_children_with(self);
    }

    fn visit_catch_clause(&mut self, catch: &swc_core::ecma::ast::CatchClause) {
        let params = catch
            .param
            .as_ref()
            .and_then(Self::short_param)
            .map(|binding| {
                let mut param = RoleParam::new(binding, "error");
                param.require_read = true;
                param
            })
            .into_iter()
            .collect();
        self.with_scope(params, |this| catch.visit_children_with(this));
    }

    fn visit_new_expr(&mut self, new: &swc_core::ecma::ast::NewExpr) {
        let executor = match (new.callee.as_ref(), new.args.as_deref()) {
            (Expr::Ident(callee), Some([first, ..]))
                if first.spread.is_none()
                    && is_unresolved_ident(callee, "Promise", self.unresolved_mark) =>
            {
                Self::callback_params(&first.expr).map(|params| (params, first.expr.as_ref()))
            }
            _ => None,
        };
        let Some((params, executor)) = executor else {
            new.visit_children_with(self);
            return;
        };
        let roles = params
            .iter()
            .zip(["resolve", "reject"])
            .filter_map(|(pat, role)| Self::short_param(pat).map(|b| RoleParam::new(b, role)))
            .collect();
        new.callee.visit_with(self);
        self.with_scope(roles, |this| executor.visit_with(this));
        for arg in new.args.iter().flatten().skip(1) {
            arg.visit_with(self);
        }
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        let reduce = match (&call.callee, call.args.first()) {
            (Callee::Expr(callee), Some(first)) if first.spread.is_none() => {
                match callee.as_ref() {
                    Expr::Member(MemberExpr {
                        obj,
                        prop: MemberProp::Ident(prop),
                        ..
                    }) if matches!(prop.sym.as_ref(), "reduce" | "reduceRight") => {
                        Self::callback_params(&first.expr)
                            .map(|params| (obj.as_ref(), params, first.expr.as_ref()))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        let Some((receiver, params, callback)) = reduce else {
            call.visit_children_with(self);
            return;
        };
        let element = self.reduce_element_name(receiver);
        let roles = params
            .iter()
            .zip(["acc", element.as_str(), "index"])
            .enumerate()
            .filter_map(|(position, (pat, role))| {
                Self::short_param(pat).map(|b| {
                    let mut param = RoleParam::new(b, role);
                    param.forbid_writes = position != 0;
                    param
                })
            })
            .collect();
        call.callee.visit_with(self);
        self.with_scope(roles, |this| callback.visit_with(this));
        for arg in call.args.iter().skip(1) {
            arg.visit_with(self);
        }
    }

    fn visit_for_stmt(&mut self, for_stmt: &ForStmt) {
        let params = Self::loop_element_params(for_stmt);
        self.with_scope(params, |this| for_stmt.visit_children_with(this));
    }
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

fn sentry_component_rename_module(module: &mut Module, exported_bindings: &HashSet<BindingId>) {
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
        if exported_bindings.contains(&bid) {
            continue;
        }
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

    strip_sentry_rename_attrs(module);
}

/// Strip `data-sentry-component` and `data-sentry-element` attributes from
/// JSX elements after SmartRename has consumed them. These are build-tool
/// artifacts, not original source code.
fn strip_sentry_rename_attrs(module: &mut Module) {
    module.visit_mut_with(&mut SentryRenameAttrStripper);
}

struct SentryRenameAttrStripper;

impl VisitMut for SentryRenameAttrStripper {
    fn visit_mut_jsx_opening_element(&mut self, elem: &mut swc_core::ecma::ast::JSXOpeningElement) {
        elem.attrs.retain(|attr| {
            let JSXAttrOrSpread::JSXAttr(jsx_attr) = attr else {
                return true;
            };
            let JSXAttrName::Ident(name) = &jsx_attr.name else {
                return true;
            };
            let n = name.sym.as_ref();
            !SENTRY_ATTR_NAMES.contains(&n) && !SENTRY_ELEMENT_ATTR_NAMES.contains(&n)
        });
        elem.visit_mut_children_with(self);
    }
}

/// Strip `data-sentry-source-file` from all JSX elements in the module, but
/// only when every occurrence carries the same value AND that value matches
/// the module's output filename (stem). When values differ — e.g. in a
/// scope-concatenated module that merged multiple original files — the
/// markers are kept as file-boundary hints for the reader.
pub fn strip_redundant_sentry_source_file(module: &mut Module, filename: &str) {
    let stem = filename_stem(filename);
    let mut collector = SourceFileValueCollector::default();
    module.visit_with(&mut collector);
    if collector.values.is_empty() {
        return;
    }
    if collector.values.len() != 1 {
        return;
    }
    let sole_value = collector.values.into_iter().next().unwrap();
    let marker_stem = filename_stem(&sole_value);
    if marker_stem != stem {
        return;
    }
    module.visit_mut_with(&mut SentrySourceFileStripper);
}

fn filename_stem(filename: &str) -> &str {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    base.rsplit_once('.').map_or(base, |(stem, _)| stem)
}

#[derive(Default)]
struct SourceFileValueCollector {
    values: HashSet<String>,
}

impl Visit for SourceFileValueCollector {
    fn visit_jsx_opening_element(&mut self, elem: &swc_core::ecma::ast::JSXOpeningElement) {
        for attr in &elem.attrs {
            let JSXAttrOrSpread::JSXAttr(jsx_attr) = attr else {
                continue;
            };
            let JSXAttrName::Ident(name) = &jsx_attr.name else {
                continue;
            };
            if !SENTRY_SOURCE_FILE_ATTR_NAMES.contains(&name.sym.as_ref()) {
                continue;
            }
            if let Some(JSXAttrValue::Str(s)) = &jsx_attr.value {
                if let Some(val) = s.value.as_str() {
                    self.values.insert(val.to_string());
                }
            }
        }
        elem.visit_children_with(self);
    }
}

struct SentrySourceFileStripper;

impl VisitMut for SentrySourceFileStripper {
    fn visit_mut_jsx_opening_element(&mut self, elem: &mut swc_core::ecma::ast::JSXOpeningElement) {
        elem.attrs.retain(|attr| {
            let JSXAttrOrSpread::JSXAttr(jsx_attr) = attr else {
                return true;
            };
            let JSXAttrName::Ident(name) = &jsx_attr.name else {
                return true;
            };
            !SENTRY_SOURCE_FILE_ATTR_NAMES.contains(&name.sym.as_ref())
        });
        elem.visit_mut_children_with(self);
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
        .rsplit(['/', '\\'])
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

fn jsx_component_alias_rename_module(module: &mut Module, exported_bindings: &HashSet<BindingId>) {
    let mut collector = JsxComponentAliasCollector::default();
    module.visit_with(&mut collector);
    if collector.aliases.is_empty() {
        return;
    }

    let mut classifier = JsxComponentAliasClassifier::new(collector.aliases);
    module.visit_with(&mut classifier);

    let eligible: Vec<_> = classifier
        .states
        .into_iter()
        .filter(|(bid, state)| {
            !exported_bindings.contains(bid) && state.other_uses == 0 && state.jsx_uses > 0
        })
        .collect();
    let mut target_counts = HashMap::default();
    for (_, state) in &eligible {
        *target_counts.entry(state.target.clone()).or_insert(0usize) += 1;
    }

    let mut renames = Vec::new();
    for (bid, state) in eligible {
        // A source-derived name is not discriminative when multiple aliases
        // would choose it. Renaming any of them would either collide or make
        // the result depend on HashMap iteration order.
        if target_counts.get(state.target.as_str()) != Some(&1) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_non_conflicting_name_uses_next_available_suffix() {
        let used_names = HashSet::from_iter([
            Atom::from("rest"),
            Atom::from("rest_1"),
            Atom::from("rest_2"),
        ]);

        assert_eq!(find_non_conflicting_name("rest", &used_names), "rest_3");
    }
}
