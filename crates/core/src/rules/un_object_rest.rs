use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignPat, AssignPatProp, BinaryOp, BindingIdent, BlockStmtOrExpr, Bool, CallExpr,
    Callee, CondExpr, Decl, Expr, ExprStmt, FnExpr, Ident, KeyValuePatProp, Lit, MemberExpr,
    MemberProp, ObjectPat, ObjectPatProp, Pat, PropName, PropOrSpread, RestPat, Stmt, VarDecl,
    VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::babel_helper_utils::{
    remove_helpers_without_remaining_refs, tslib_member_helper_kind, BabelHelperKind, BindingKey,
    LocalHelperContext,
};

use crate::utils::paren::strip_parens;

/// Convert inline `_objectWithoutPropertiesLoose` IIFEs to object rest destructuring.
///
/// ```js
/// const rest = ((e, t) => {
///     const n = {};
///     for (const r in e) {
///         t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
///     }
///     return n;
/// })(obj, ["a", "b"]);
/// // →
/// const { a, b, ...rest } = obj;
/// ```
pub struct UnObjectRest {
    unresolved_mark: Mark,
}

impl UnObjectRest {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }

    pub(crate) fn run_with_helpers(
        module: &mut swc_core::ecma::ast::Module,
        unresolved_mark: Mark,
        local_helpers: &LocalHelperContext,
    ) {
        run_un_object_rest(module, unresolved_mark, local_helpers);
    }
}

impl UnObjectRest {
    fn has_owp_iife_candidate(module: &swc_core::ecma::ast::Module) -> bool {
        struct Scan {
            found: bool,
        }
        impl Visit for Scan {
            fn visit_call_expr(&mut self, call: &CallExpr) {
                if self.found {
                    return;
                }
                if call.args.len() == 2
                    && call.args.iter().all(|a| a.spread.is_none())
                    && matches!(call.args[1].expr.as_ref(), Expr::Array(_))
                {
                    let callee_is_fn = if let Callee::Expr(e) = &call.callee {
                        match strip_parens(e) {
                            Expr::Arrow(a) => a.params.len() == 2,
                            Expr::Fn(f) => f.function.params.len() == 2,
                            _ => false,
                        }
                    } else {
                        false
                    };
                    if callee_is_fn {
                        self.found = true;
                        return;
                    }
                }
                call.visit_children_with(self);
            }
        }
        let mut scan = Scan { found: false };
        module.visit_with(&mut scan);
        scan.found
    }
}

impl VisitMut for UnObjectRest {
    fn visit_mut_module(&mut self, module: &mut swc_core::ecma::ast::Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_object_rest(module, self.unresolved_mark, &local_helpers);
    }
}

fn run_un_object_rest(
    module: &mut swc_core::ecma::ast::Module,
    unresolved_mark: Mark,
    local_helpers: &LocalHelperContext,
) {
    let named_helpers = local_helpers.helpers_of_kind(BabelHelperKind::ObjectWithoutProperties);
    let tslib_namespaces = local_helpers.tslib_namespaces();

    if named_helpers.is_empty()
        && tslib_namespaces.is_empty()
        && !UnObjectRest::has_owp_iife_candidate(module)
    {
        return;
    }

    let mut helper_dependencies = local_helpers.helpers_of_kind(BabelHelperKind::HelperDependency);
    helper_dependencies.extend(local_helpers.helpers_of_kind(BabelHelperKind::DefineProperty));

    // Process inner scopes first (function bodies, etc.) with helpers available
    let mut processor = ObjectRestProcessor {
        named_helpers: &named_helpers,
        tslib_namespaces,
        unresolved_mark,
    };
    module.visit_mut_children_with(&mut processor);
    reattach_elided_object_rest_in_module_items(
        &mut module.body,
        &named_helpers,
        tslib_namespaces,
        unresolved_mark,
    );

    // Process module-level statements
    let mut new_body = Vec::with_capacity(module.body.len());
    let mut recent_stmts: Vec<Stmt> = Vec::new();

    for item in std::mem::take(&mut module.body) {
        let ModuleItem::Stmt(ref stmt) = item else {
            recent_stmts.clear();
            new_body.push(item);
            continue;
        };

        let extraction = try_extract_owp_iife(stmt)
            .or_else(|| try_extract_owp_named_call(stmt, &named_helpers, tslib_namespaces));

        if let Some((rest_binding, source, excluded_keys, before, after)) = extraction {
            let mut inline_accesses = declarators_to_accesses(&before, &source, &excluded_keys);
            let (absorbed, mut preceding_accesses) =
                scan_preceding(&recent_stmts, &source, &excluded_keys, unresolved_mark);
            for _ in 0..absorbed {
                recent_stmts.pop();
                new_body.pop();
            }
            preceding_accesses.append(&mut inline_accesses);
            let scope_names = collect_scope_names_module(&new_body);
            let new_stmt = build_rest_destructuring(
                &rest_binding,
                &source,
                &excluded_keys,
                &preceding_accesses,
                &scope_names,
            );
            recent_stmts.push(new_stmt.clone());
            new_body.push(ModuleItem::Stmt(new_stmt));
            if !after.is_empty() {
                let after_stmt = Stmt::Decl(Decl::Var(Box::new(VarDecl {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    kind: VarDeclKind::Var,
                    declare: false,
                    decls: after,
                })));
                recent_stmts.push(after_stmt.clone());
                new_body.push(ModuleItem::Stmt(after_stmt));
            }
            continue;
        }

        recent_stmts.push(stmt.clone());
        new_body.push(item);
    }
    module.body = new_body;

    // Remove named helper declarations if all call sites were replaced
    if !named_helpers.is_empty() {
        let removable_helpers = named_helpers
            .iter()
            .chain(helper_dependencies.iter())
            .map(|(key, kind)| (key.clone(), *kind))
            .collect::<HashMap<_, _>>();
        remove_helpers_without_remaining_refs(module, removable_helpers);
    }
}

struct ObjectRestProcessor<'a> {
    named_helpers: &'a HashMap<BindingKey, BabelHelperKind>,
    tslib_namespaces: &'a HashSet<BindingKey>,
    unresolved_mark: Mark,
}

impl VisitMut for ObjectRestProcessor<'_> {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        reattach_elided_object_rest_in_stmts(
            stmts,
            self.named_helpers,
            self.tslib_namespaces,
            self.unresolved_mark,
        );

        let mut new_stmts = Vec::with_capacity(stmts.len());

        for stmt in stmts.iter() {
            let extraction = try_extract_owp_iife(stmt).or_else(|| {
                try_extract_owp_named_call(stmt, self.named_helpers, self.tslib_namespaces)
            });

            if let Some((rest_binding, source, excluded_keys, before, after)) = extraction {
                let mut inline_accesses = declarators_to_accesses(&before, &source, &excluded_keys);
                let (absorbed, mut preceding_accesses) =
                    scan_preceding(&new_stmts, &source, &excluded_keys, self.unresolved_mark);
                for _ in 0..absorbed {
                    new_stmts.pop();
                }
                preceding_accesses.append(&mut inline_accesses);
                let scope_names = collect_scope_names(&new_stmts);
                new_stmts.push(build_rest_destructuring(
                    &rest_binding,
                    &source,
                    &excluded_keys,
                    &preceding_accesses,
                    &scope_names,
                ));
                if !after.is_empty() {
                    new_stmts.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                        span: DUMMY_SP,
                        ctxt: Default::default(),
                        kind: VarDeclKind::Var,
                        declare: false,
                        decls: after,
                    }))));
                }
                continue;
            }

            new_stmts.push(stmt.clone());
        }

        *stmts = new_stmts;
    }
}

use swc_core::ecma::ast::ModuleItem;

fn reattach_elided_object_rest_in_module_items(
    items: &mut [ModuleItem],
    named_helpers: &HashMap<BindingKey, BabelHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) {
    if !module_items_contain_owp_spread_candidate(items, named_helpers, tslib_namespaces) {
        return;
    }

    for item in items.iter_mut() {
        if let ModuleItem::Stmt(stmt) = item {
            reattach_elided_object_rest_in_stmt(
                stmt,
                named_helpers,
                tslib_namespaces,
                unresolved_mark,
            );
        }
    }

    for rest_idx in 0..items.len() {
        let Some(rest_binding) = module_item_single_undefined_binding(&items[rest_idx]) else {
            continue;
        };
        let preceding: Vec<Stmt> = items[..rest_idx]
            .iter()
            .filter_map(|item| match item {
                ModuleItem::Stmt(stmt) => Some(stmt.clone()),
                ModuleItem::ModuleDecl(_) => None,
            })
            .collect();

        let mut replacement_init = None;
        for item in items.iter_mut().skip(rest_idx + 1) {
            let ModuleItem::Stmt(stmt) = item else {
                continue;
            };
            let mut replacer = ElidedRestSpreadReplacer {
                rest_binding: &rest_binding,
                named_helpers,
                tslib_namespaces,
                preceding: Some(&preceding),
                unresolved_mark,
                replacement_init: None,
            };
            stmt.visit_mut_with(&mut replacer);
            if replacer.replacement_init.is_some() {
                replacement_init = replacer.replacement_init;
                break;
            }
        }

        if let Some(init) = replacement_init {
            set_module_item_single_decl_init(&mut items[rest_idx], init);
        }
    }
}

fn reattach_elided_object_rest_in_stmts(
    stmts: &mut [Stmt],
    named_helpers: &HashMap<BindingKey, BabelHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) {
    if !stmts_contain_owp_spread_candidate(stmts, named_helpers, tslib_namespaces) {
        return;
    }

    for stmt in stmts.iter_mut() {
        reattach_elided_object_rest_in_stmt(stmt, named_helpers, tslib_namespaces, unresolved_mark);
    }

    for rest_idx in 0..stmts.len() {
        let Some(rest_binding) = stmt_single_undefined_binding(&stmts[rest_idx]) else {
            continue;
        };
        let preceding = stmts[..rest_idx].to_vec();

        let mut replacement_init = None;
        for stmt in stmts.iter_mut().skip(rest_idx + 1) {
            let mut replacer = ElidedRestSpreadReplacer {
                rest_binding: &rest_binding,
                named_helpers,
                tslib_namespaces,
                preceding: Some(&preceding),
                unresolved_mark,
                replacement_init: None,
            };
            stmt.visit_mut_with(&mut replacer);
            if replacer.replacement_init.is_some() {
                replacement_init = replacer.replacement_init;
                break;
            }
        }

        if let Some(init) = replacement_init {
            set_stmt_single_decl_init(&mut stmts[rest_idx], init);
        }
    }
}

fn module_items_contain_owp_spread_candidate(
    items: &[ModuleItem],
    named_helpers: &HashMap<BindingKey, BabelHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) -> bool {
    let mut visitor = ObjectRestSpreadCandidateVisitor {
        named_helpers,
        tslib_namespaces,
        found: false,
    };
    for item in items {
        item.visit_with(&mut visitor);
        if visitor.found {
            return true;
        }
    }
    false
}

fn stmts_contain_owp_spread_candidate(
    stmts: &[Stmt],
    named_helpers: &HashMap<BindingKey, BabelHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) -> bool {
    let mut visitor = ObjectRestSpreadCandidateVisitor {
        named_helpers,
        tslib_namespaces,
        found: false,
    };
    for stmt in stmts {
        stmt.visit_with(&mut visitor);
        if visitor.found {
            return true;
        }
    }
    false
}

struct ObjectRestSpreadCandidateVisitor<'a> {
    named_helpers: &'a HashMap<BindingKey, BabelHelperKind>,
    tslib_namespaces: &'a HashSet<BindingKey>,
    found: bool,
}

impl Visit for ObjectRestSpreadCandidateVisitor<'_> {
    fn visit_prop_or_spread(&mut self, prop: &PropOrSpread) {
        if self.found {
            return;
        }

        let PropOrSpread::Spread(spread) = prop else {
            prop.visit_children_with(self);
            return;
        };

        if extract_named_owp_args(&spread.expr, self.named_helpers, self.tslib_namespaces)
            .or_else(|| try_extract_owp_call(&spread.expr))
            .is_some()
        {
            self.found = true;
            return;
        }

        spread.expr.visit_with(self);
    }
}

fn reattach_elided_object_rest_in_stmt(
    stmt: &mut Stmt,
    named_helpers: &HashMap<BindingKey, BabelHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return;
    };

    for rest_idx in 0..var.decls.len() {
        let Some(rest_binding) = undefined_ident_declarator(&var.decls[rest_idx]) else {
            continue;
        };

        let mut replacement_init = None;
        for decl in var.decls.iter_mut().skip(rest_idx + 1) {
            let mut replacer = ElidedRestSpreadReplacer {
                rest_binding: &rest_binding,
                named_helpers,
                tslib_namespaces,
                preceding: None,
                unresolved_mark,
                replacement_init: None,
            };
            decl.visit_mut_with(&mut replacer);
            if replacer.replacement_init.is_some() {
                replacement_init = replacer.replacement_init;
                break;
            }
        }

        if let Some(init) = replacement_init {
            var.decls[rest_idx].init = Some(init);
            break;
        }
    }
}

struct ElidedRestSpreadReplacer<'a> {
    rest_binding: &'a BindingIdent,
    named_helpers: &'a HashMap<BindingKey, BabelHelperKind>,
    tslib_namespaces: &'a HashSet<BindingKey>,
    preceding: Option<&'a [Stmt]>,
    unresolved_mark: Mark,
    replacement_init: Option<Box<Expr>>,
}

impl VisitMut for ElidedRestSpreadReplacer<'_> {
    fn visit_mut_prop_or_spread(&mut self, prop: &mut PropOrSpread) {
        if self.replacement_init.is_some() {
            return;
        }

        let PropOrSpread::Spread(spread) = prop else {
            prop.visit_mut_children_with(self);
            return;
        };

        let extraction =
            extract_named_owp_args(&spread.expr, self.named_helpers, self.tslib_namespaces)
                .or_else(|| try_extract_owp_call(&spread.expr));
        let Some((source, excluded_keys)) = extraction else {
            spread.visit_mut_children_with(self);
            return;
        };

        if let Some(preceding) = self.preceding {
            let (absorbed, _) =
                scan_preceding(preceding, &source, &excluded_keys, self.unresolved_mark);
            if absorbed == 0 {
                spread.visit_mut_children_with(self);
                return;
            }
        }

        self.replacement_init = Some(spread.expr.clone());
        *spread.expr = Expr::Ident(self.rest_binding.id.clone());
    }
}

fn module_item_single_undefined_binding(item: &ModuleItem) -> Option<BindingIdent> {
    let ModuleItem::Stmt(stmt) = item else {
        return None;
    };
    stmt_single_undefined_binding(stmt)
}

fn stmt_single_undefined_binding(stmt: &Stmt) -> Option<BindingIdent> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    let [decl] = var.decls.as_slice() else {
        return None;
    };
    undefined_ident_declarator(decl)
}

fn undefined_ident_declarator(decl: &VarDeclarator) -> Option<BindingIdent> {
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    match &decl.init {
        Some(init) if is_undefined_expr(init) => Some(binding.clone()),
        None => Some(binding.clone()),
        _ => None,
    }
}

fn set_module_item_single_decl_init(item: &mut ModuleItem, init: Box<Expr>) {
    if let ModuleItem::Stmt(stmt) = item {
        set_stmt_single_decl_init(stmt, init);
    }
}

fn set_stmt_single_decl_init(stmt: &mut Stmt, init: Box<Expr>) {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return;
    };
    let [decl] = var.decls.as_mut_slice() else {
        return;
    };
    decl.init = Some(init);
}

fn is_undefined_expr(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Ident(id) if id.sym.as_ref() == "undefined")
}

/// Extracted info from a preceding statement that accesses the same source object.
enum PrecedingAccess {
    /// `const { a, b: c } = source` — destructuring with key→binding pairs
    Destructuring(Vec<(Atom, Atom, SyntaxContext, Option<Box<Expr>>)>), // (prop_key, local_binding, binding_ctxt, default)
    /// `const x = source.prop` — single property access
    PropAccess {
        prop: Atom,
        binding: Atom,
        ctxt: SyntaxContext,
    },
    /// Two-statement pair: `const tmp = source.prop; const x = tmp === undefined ? def : tmp`
    PropAccessWithDefault {
        prop: Atom,
        binding: Atom,
        ctxt: SyntaxContext,
        default_value: Box<Expr>,
    },
    /// `source.prop;` — bare access (no binding)
    BareAccess { _prop: Atom },
}

/// Try to extract an `_objectWithoutPropertiesLoose` inline IIFE from a statement.
/// Returns (rest_binding_name, source_expr, excluded_keys, declarators_before, declarators_after).
/// The before/after declarators are from the same var decl if it had multiple declarators.
fn try_extract_owp_iife(
    stmt: &Stmt,
) -> Option<(
    BindingIdent,
    Box<Expr>,
    Vec<Atom>,
    Vec<VarDeclarator>,
    Vec<VarDeclarator>,
)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };

    // Find the first declarator whose init is an OWP IIFE
    let owp_idx = var.decls.iter().position(|decl| {
        let Pat::Ident(_) = &decl.name else {
            return false;
        };
        let Some(init) = &decl.init else {
            return false;
        };
        try_extract_owp_call(init).is_some()
    })?;

    let decl = &var.decls[owp_idx];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let (source, excluded_keys) = try_extract_owp_call(init)?;

    let before = var.decls[..owp_idx].to_vec();
    let after = var.decls[owp_idx + 1..].to_vec();
    Some((binding.clone(), source, excluded_keys, before, after))
}

/// Try to extract a named OWP helper call from a statement.
/// Matches: `const rest = helperName(source, ["key1", "key2"])`
fn try_extract_owp_named_call(
    stmt: &Stmt,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) -> Option<(
    BindingIdent,
    Box<Expr>,
    Vec<Atom>,
    Vec<VarDeclarator>,
    Vec<VarDeclarator>,
)> {
    if helpers.is_empty() && tslib_namespaces.is_empty() {
        return None;
    }
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };

    let owp_idx = var.decls.iter().position(|decl| {
        let Pat::Ident(_) = &decl.name else {
            return false;
        };
        let Some(init) = &decl.init else {
            return false;
        };
        extract_named_owp_args(init, helpers, tslib_namespaces).is_some()
    })?;

    let decl = &var.decls[owp_idx];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let (source, excluded_keys) = extract_named_owp_args(init, helpers, tslib_namespaces)?;

    let before = var.decls[..owp_idx].to_vec();
    let after = var.decls[owp_idx + 1..].to_vec();
    Some((binding.clone(), source, excluded_keys, before, after))
}

/// Extract (source, excluded_keys) from a call to a known named OWP helper.
fn extract_named_owp_args(
    expr: &Expr,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) -> Option<(Box<Expr>, Vec<Atom>)> {
    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee),
        args,
        ..
    }) = expr
    else {
        return None;
    };
    let is_helper = match callee.as_ref() {
        Expr::Ident(id) => helpers.contains_key(&(id.sym.clone(), id.ctxt)),
        Expr::Member(_) => matches!(
            tslib_member_helper_kind(callee, tslib_namespaces),
            Some(BabelHelperKind::ObjectWithoutProperties)
        ),
        _ => false,
    };
    if !is_helper {
        return None;
    }
    if args.len() != 2 || args[0].spread.is_some() || args[1].spread.is_some() {
        return None;
    }
    let Expr::Array(arr) = args[1].expr.as_ref() else {
        return None;
    };
    let mut keys: Vec<Atom> = Vec::new();
    for elem in &arr.elems {
        let Some(elem) = elem else { return None };
        if elem.spread.is_some() {
            return None;
        }
        let Expr::Lit(Lit::Str(s)) = elem.expr.as_ref() else {
            return None;
        };
        let key_str = s.value.as_str()?;
        keys.push(Atom::from(key_str));
    }
    Some((args[0].expr.clone(), keys))
}

/// Check if an expression is an OWP IIFE call, returning (source, excluded_keys).
fn try_extract_owp_call(expr: &Expr) -> Option<(Box<Expr>, Vec<Atom>)> {
    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee),
        args,
        ..
    }) = expr
    else {
        return None;
    };
    if args.len() != 2 || args[0].spread.is_some() || args[1].spread.is_some() {
        return None;
    }
    let Expr::Array(arr) = args[1].expr.as_ref() else {
        return None;
    };
    let mut keys: Vec<Atom> = Vec::new();
    for elem in &arr.elems {
        let Some(elem) = elem else { return None };
        if elem.spread.is_some() {
            return None;
        }
        let Expr::Lit(Lit::Str(s)) = elem.expr.as_ref() else {
            return None;
        };
        let key_str = s.value.as_str()?;
        keys.push(Atom::from(key_str));
    }
    let callee = strip_parens(callee);
    let body_stmts = match callee {
        Expr::Arrow(ArrowExpr { body, params, .. }) if params.len() == 2 => match &**body {
            BlockStmtOrExpr::BlockStmt(block) => &block.stmts,
            _ => return None,
        },
        Expr::Fn(FnExpr { function, .. }) if function.params.len() == 2 => {
            function.body.as_ref()?.stmts.as_slice()
        }
        _ => return None,
    };
    if !is_owp_body(body_stmts) {
        return None;
    }
    Some((args[0].expr.clone(), keys))
}

/// Check if function body matches the objectWithoutPropertiesLoose shape:
/// ```js
/// const/var n = {};
/// for (const/var r in e) {
///     t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
/// }
/// return n;
/// ```
fn is_owp_body(stmts: &[Stmt]) -> bool {
    // 3 statements: var init, for-in, return
    if stmts.len() != 3 {
        return false;
    }

    // First: var/const n = {}
    let Stmt::Decl(Decl::Var(var)) = &stmts[0] else {
        return false;
    };
    if var.decls.len() != 1 {
        return false;
    }
    let Some(init) = &var.decls[0].init else {
        return false;
    };
    if !matches!(init.as_ref(), Expr::Object(obj) if obj.props.is_empty()) {
        return false;
    }

    // Second: for (... in ...) with indexOf + hasOwnProperty in body
    let Stmt::ForIn(for_in) = &stmts[1] else {
        return false;
    };
    if !for_in_body_has_owp_shape(&for_in.body) {
        return false;
    }

    // Third: return <ident> (the accumulator)
    let Stmt::Return(ret) = &stmts[2] else {
        return false;
    };
    matches!(&ret.arg, Some(arg) if matches!(arg.as_ref(), Expr::Ident(_)))
}

/// Scan backward from the end of `preceding` for statements that access `source`.
/// Returns (count_absorbed, merged_prop_info).
fn scan_preceding(
    preceding: &[Stmt],
    source: &Expr,
    excluded_keys: &[Atom],
    unresolved_mark: Mark,
) -> (usize, Vec<PrecedingAccess>) {
    let source_name = match source {
        Expr::Ident(id) => &id.sym,
        _ => return (0, vec![]),
    };

    let mut absorbed = 0;
    let mut accesses = Vec::new();
    let mut idx = preceding.len();

    while idx > 0 {
        idx -= 1;
        let stmt = &preceding[idx];

        if let Some(access) = try_match_preceding(stmt, source_name, excluded_keys) {
            absorbed += 1;
            accesses.push(access);
            continue;
        }

        // Two-statement pair: ternary default (current) + extraction (previous)
        if idx > 0 {
            if let Some(access) = try_match_default_pair(
                &preceding[idx - 1],
                stmt,
                source_name,
                excluded_keys,
                unresolved_mark,
            ) {
                absorbed += 2;
                idx -= 1;
                accesses.push(access);
                continue;
            }
        }

        break;
    }

    accesses.reverse();
    (absorbed, accesses)
}

/// Convert preceding declarators from the same var decl to PrecedingAccess entries.
/// Handles `t = e.to` → PropAccess and `e["aria-current"]` → PropAccess with string key.
fn declarators_to_accesses(
    decls: &[VarDeclarator],
    source: &Expr,
    excluded_keys: &[Atom],
) -> Vec<PrecedingAccess> {
    let source_name = match source {
        Expr::Ident(id) => &id.sym,
        _ => return vec![],
    };
    let mut accesses = Vec::new();
    for decl in decls {
        let Pat::Ident(bi) = &decl.name else {
            continue;
        };
        let Some(init) = &decl.init else {
            continue;
        };
        if let Expr::Member(MemberExpr { obj, prop, .. }) = init.as_ref() {
            if let Expr::Ident(obj_id) = obj.as_ref() {
                if obj_id.sym == *source_name {
                    let prop_name = match prop {
                        MemberProp::Ident(id) => Some(id.sym.clone()),
                        MemberProp::Computed(c) => {
                            if let Expr::Lit(Lit::Str(s)) = c.expr.as_ref() {
                                s.value.as_str().map(Atom::from)
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some(prop_name) = prop_name {
                        if excluded_keys.contains(&prop_name) {
                            accesses.push(PrecedingAccess::PropAccess {
                                prop: prop_name,
                                binding: bi.id.sym.clone(),
                                ctxt: bi.id.ctxt,
                            });
                        }
                    }
                }
            }
        }
    }
    accesses
}

fn try_match_preceding(
    stmt: &Stmt,
    source_name: &Atom,
    excluded_keys: &[Atom],
) -> Option<PrecedingAccess> {
    // Case 1: const { a, b } = source
    if let Stmt::Decl(Decl::Var(var)) = stmt {
        if var.decls.len() == 1 {
            let decl = &var.decls[0];
            if let Pat::Object(obj_pat) = &decl.name {
                if let Some(init) = &decl.init {
                    if let Expr::Ident(id) = init.as_ref() {
                        if id.sym == *source_name {
                            let mut pairs = Vec::new();
                            for prop in &obj_pat.props {
                                match prop {
                                    ObjectPatProp::Assign(a) => {
                                        let key = a.key.id.sym.clone();
                                        if excluded_keys.contains(&key) {
                                            pairs.push((
                                                key.clone(),
                                                key,
                                                a.key.id.ctxt,
                                                a.value.clone(),
                                            ));
                                        }
                                    }
                                    ObjectPatProp::KeyValue(kv) => {
                                        let key = prop_name_atom(&kv.key)?;
                                        if excluded_keys.contains(&key) {
                                            if let Pat::Ident(bi) = kv.value.as_ref() {
                                                pairs.push((
                                                    key,
                                                    bi.id.sym.clone(),
                                                    bi.id.ctxt,
                                                    None,
                                                ));
                                            } else if let Pat::Assign(assign) = kv.value.as_ref() {
                                                if let Pat::Ident(bi) = assign.left.as_ref() {
                                                    pairs.push((
                                                        key,
                                                        bi.id.sym.clone(),
                                                        bi.id.ctxt,
                                                        Some(assign.right.clone()),
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            if !pairs.is_empty() {
                                return Some(PrecedingAccess::Destructuring(pairs));
                            }
                        }
                    }
                }
            }

            // Case 2: const x = source.prop
            if let Pat::Ident(bi) = &decl.name {
                if let Some(init) = &decl.init {
                    if let Expr::Member(MemberExpr { obj, prop, .. }) = init.as_ref() {
                        if let Expr::Ident(obj_id) = obj.as_ref() {
                            if obj_id.sym == *source_name {
                                if let Some(pname) = member_prop_atom(prop) {
                                    if excluded_keys.contains(&pname) {
                                        return Some(PrecedingAccess::PropAccess {
                                            prop: pname,
                                            binding: bi.id.sym.clone(),
                                            ctxt: bi.id.ctxt,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Case 3: source.prop; (bare expression statement)
    if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
        if let Expr::Member(MemberExpr { obj, prop, .. }) = expr.as_ref() {
            if let Expr::Ident(obj_id) = obj.as_ref() {
                if obj_id.sym == *source_name {
                    if let Some(pname) = member_prop_atom(prop) {
                        if excluded_keys.contains(&pname) {
                            return Some(PrecedingAccess::BareAccess { _prop: pname });
                        }
                    }
                }
            }
        }
    }

    None
}

/// Try to match a two-statement pair:
///   extraction: `const tmp = source.prop`
///   default:    one of these forms:
///     - `const x = tmp === undefined ? defaultVal : tmp`  (ternary)
///     - `const x = tmp === undefined || tmp`              (boolean default true)
///     - `const x = tmp !== undefined && tmp`              (boolean default false)
fn try_match_default_pair(
    extraction_stmt: &Stmt,
    default_stmt: &Stmt,
    source_name: &Atom,
    excluded_keys: &[Atom],
    unresolved_mark: Mark,
) -> Option<PrecedingAccess> {
    // 1. Parse the default stmt
    let (final_binding, tmp_name, default_value) =
        extract_default_assignment(default_stmt, unresolved_mark)?;

    // 2. Parse the extraction stmt: const tmp = source.prop
    let Stmt::Decl(Decl::Var(extract_var)) = extraction_stmt else {
        return None;
    };
    if extract_var.decls.len() != 1 {
        return None;
    }
    let extract_decl = &extract_var.decls[0];
    let Pat::Ident(extract_binding) = &extract_decl.name else {
        return None;
    };
    if extract_binding.id.sym != tmp_name {
        return None;
    }
    let Some(extract_init) = &extract_decl.init else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = extract_init.as_ref() else {
        return None;
    };
    let Expr::Ident(obj_id) = obj.as_ref() else {
        return None;
    };
    if obj_id.sym != *source_name {
        return None;
    }
    let prop_name = member_prop_atom(prop)?;
    if !excluded_keys.contains(&prop_name) {
        return None;
    }

    match default_value {
        None => Some(PrecedingAccess::PropAccess {
            prop: prop_name,
            binding: final_binding.id.sym.clone(),
            ctxt: final_binding.id.ctxt,
        }),
        Some(def) => Some(PrecedingAccess::PropAccessWithDefault {
            prop: prop_name,
            binding: final_binding.id.sym.clone(),
            ctxt: final_binding.id.ctxt,
            default_value: def,
        }),
    }
}

/// Extract a default-value assignment from a var declaration.
/// Returns (final_binding, tmp_variable_name, default_expr_or_none).
///
/// Matches three forms:
/// 1. `const x = tmp === undefined ? defaultVal : tmp` → Some(defaultVal)
///    (when defaultVal is `undefined` itself → None, since destructuring
///    naturally produces undefined for missing properties)
/// 2. `const x = tmp === undefined || tmp` → Some(true)
/// 3. `const x = tmp !== undefined && tmp` → Some(false)
fn extract_default_assignment(
    stmt: &Stmt,
    unresolved_mark: Mark,
) -> Option<(BindingIdent, Atom, Option<Box<Expr>>)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(final_binding) = &decl.name else {
        return None;
    };
    let Some(init) = &decl.init else {
        return None;
    };

    match init.as_ref() {
        // Form 1: tmp === undefined ? defaultVal : tmp
        Expr::Cond(CondExpr {
            test, cons, alt, ..
        }) => {
            let (tmp_name, _) = match_undefined_check(test, BinaryOp::EqEqEq, unresolved_mark)?;
            if !matches!(alt.as_ref(), Expr::Ident(id) if id.sym == tmp_name) {
                return None;
            }
            let default_value = if matches!(cons.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "undefined" && (id.ctxt.outer() == unresolved_mark || id.ctxt == SyntaxContext::empty()))
            {
                None
            } else {
                Some(cons.clone())
            };
            Some((final_binding.clone(), tmp_name, default_value))
        }
        // Form 2: tmp === undefined || tmp  →  default true
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalOr => {
            let (tmp_name, _) =
                match_undefined_check(&bin.left, BinaryOp::EqEqEq, unresolved_mark)?;
            if !matches!(bin.right.as_ref(), Expr::Ident(id) if id.sym == tmp_name) {
                return None;
            }
            let default_value = Box::new(Expr::Lit(Lit::Bool(Bool {
                span: DUMMY_SP,
                value: true,
            })));
            Some((final_binding.clone(), tmp_name, Some(default_value)))
        }
        // Form 3: tmp !== undefined && tmp  →  default false
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalAnd => {
            let (tmp_name, _) =
                match_undefined_check(&bin.left, BinaryOp::NotEqEq, unresolved_mark)?;
            if !matches!(bin.right.as_ref(), Expr::Ident(id) if id.sym == tmp_name) {
                return None;
            }
            let default_value = Box::new(Expr::Lit(Lit::Bool(Bool {
                span: DUMMY_SP,
                value: false,
            })));
            Some((final_binding.clone(), tmp_name, Some(default_value)))
        }
        _ => None,
    }
}

/// Match `tmp === undefined` or `tmp !== undefined` (with a specific operator).
/// Returns the tmp variable name and its SyntaxContext.
fn match_undefined_check(
    expr: &Expr,
    expected_op: BinaryOp,
    unresolved_mark: Mark,
) -> Option<(Atom, SyntaxContext)> {
    let Expr::Bin(bin) = expr else { return None };
    if bin.op != expected_op {
        return None;
    }
    let Expr::Ident(tmp) = bin.left.as_ref() else {
        return None;
    };
    // Verify `undefined` is the global, not a shadowed local binding.
    // Accept both resolver-stamped globals (outer == unresolved_mark) and
    // synthesized identifiers from RemoveVoid (SyntaxContext::empty).
    let Expr::Ident(undef_id) = bin.right.as_ref() else {
        return None;
    };
    if undef_id.sym.as_ref() != "undefined" {
        return None;
    }
    let is_global =
        undef_id.ctxt.outer() == unresolved_mark || undef_id.ctxt == SyntaxContext::empty();
    if !is_global {
        return None;
    }
    Some((tmp.sym.clone(), tmp.ctxt))
}

fn build_rest_destructuring(
    rest_binding: &BindingIdent,
    source: &Expr,
    excluded_keys: &[Atom],
    merged: &[PrecedingAccess],
    scope_names: &std::collections::HashSet<Atom>,
) -> Stmt {
    // Build a map from prop key → (local binding name, SyntaxContext) from preceding accesses.
    // Preserving the original SyntaxContext is critical so that downstream SmartRename
    // can match the destructuring binding to the body references via BindingRenamer.
    let mut key_to_binding: std::collections::HashMap<Atom, (Atom, SyntaxContext)> =
        std::collections::HashMap::new();
    let mut key_to_default: std::collections::HashMap<Atom, Box<Expr>> =
        std::collections::HashMap::new();
    for access in merged {
        match access {
            PrecedingAccess::Destructuring(pairs) => {
                for (key, binding, ctxt, default_value) in pairs {
                    key_to_binding.insert(key.clone(), (binding.clone(), *ctxt));
                    if let Some(default_value) = default_value {
                        key_to_default.insert(key.clone(), default_value.clone());
                    }
                }
            }
            PrecedingAccess::PropAccess {
                prop,
                binding,
                ctxt,
            } => {
                key_to_binding.insert(prop.clone(), (binding.clone(), *ctxt));
            }
            PrecedingAccess::PropAccessWithDefault {
                prop,
                binding,
                ctxt,
                default_value,
            } => {
                key_to_binding.insert(prop.clone(), (binding.clone(), *ctxt));
                key_to_default.insert(prop.clone(), default_value.clone());
            }
            PrecedingAccess::BareAccess { .. } => {
                // No binding — key will be included as shorthand (unused)
            }
        }
    }

    // Track generated aliases to avoid collisions between them
    let mut used_aliases: std::collections::HashSet<Atom> = std::collections::HashSet::new();

    // Build destructuring props for each excluded key
    let mut props: Vec<ObjectPatProp> = Vec::new();
    for key in excluded_keys {
        if let Some((binding, ctxt)) = key_to_binding.get(key) {
            let default_expr = key_to_default.get(key);
            let is_shorthand = *binding == *key && is_valid_ident(key);

            if is_shorthand {
                // Shorthand: { key } or { key = default }
                props.push(ObjectPatProp::Assign(AssignPatProp {
                    span: DUMMY_SP,
                    key: BindingIdent {
                        id: Ident::new(key.clone(), DUMMY_SP, *ctxt),
                        type_ann: None,
                    },
                    value: default_expr.cloned(),
                }));
            } else if let Some(def) = default_expr {
                // Aliased with default: { key: binding = default }
                props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                    key: make_prop_name(key),
                    value: Box::new(Pat::Assign(AssignPat {
                        span: DUMMY_SP,
                        left: Box::new(Pat::Ident(BindingIdent {
                            id: Ident::new(binding.clone(), DUMMY_SP, *ctxt),
                            type_ann: None,
                        })),
                        right: def.clone(),
                    })),
                }));
            } else {
                // Aliased without default: { key: binding }
                props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                    key: make_prop_name(key),
                    value: Box::new(Pat::Ident(BindingIdent {
                        id: Ident::new(binding.clone(), DUMMY_SP, *ctxt),
                        type_ann: None,
                    })),
                }));
            }
        } else {
            // Not in preceding — generate a `_key` alias
            let base = format!("_{}", key);
            let alias = find_non_conflicting_alias(&base, scope_names, &used_aliases);
            used_aliases.insert(alias.clone());
            props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                key: make_prop_name(key),
                value: Box::new(Pat::Ident(BindingIdent {
                    id: Ident::new(alias, DUMMY_SP, Default::default()),
                    type_ann: None,
                })),
            }));
        }
    }

    // Add rest element
    props.push(ObjectPatProp::Rest(RestPat {
        span: DUMMY_SP,
        dot3_token: DUMMY_SP,
        arg: Box::new(Pat::Ident(rest_binding.clone())),
        type_ann: None,
    }));

    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Object(ObjectPat {
                span: DUMMY_SP,
                props,
                optional: false,
                type_ann: None,
            }),
            init: Some(Box::new((*source).clone())),
            definite: false,
        }],
    })))
}

/// Verify the for-in body references `indexOf` and `hasOwnProperty` —
/// the defining features of `_objectWithoutPropertiesLoose`.
fn for_in_body_has_owp_shape(body: &Stmt) -> bool {
    struct MethodFinder {
        has_index_of: bool,
        has_has_own: bool,
    }

    impl Visit for MethodFinder {
        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if let MemberProp::Ident(id) = &member.prop {
                match id.sym.as_ref() {
                    "indexOf" => self.has_index_of = true,
                    "hasOwnProperty" => self.has_has_own = true,
                    _ => {}
                }
            }
            member.obj.visit_with(self);
        }
    }

    let mut finder = MethodFinder {
        has_index_of: false,
        has_has_own: false,
    };
    body.visit_with(&mut finder);
    finder.has_index_of && finder.has_has_own
}

fn member_prop_atom(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(id) => Some(id.sym.clone()),
        MemberProp::Computed(c) => {
            if let Expr::Lit(Lit::Str(s)) = c.expr.as_ref() {
                s.value.as_str().map(Atom::from)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn prop_name_atom(key: &PropName) -> Option<Atom> {
    match key {
        PropName::Ident(id) => Some(id.sym.clone()),
        PropName::Str(s) => s.value.as_str().map(Atom::from),
        _ => None,
    }
}

/// Find an alias name that doesn't collide with scope names or already-used aliases.
fn find_non_conflicting_alias(
    base: &str,
    scope_names: &std::collections::HashSet<Atom>,
    used_aliases: &std::collections::HashSet<Atom>,
) -> Atom {
    let base_atom = Atom::from(base);
    if !scope_names.contains(&base_atom) && !used_aliases.contains(&base_atom) {
        return base_atom;
    }
    for i in 1.. {
        let candidate = Atom::from(format!("{}_{}", base, i));
        if !scope_names.contains(&candidate) && !used_aliases.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Collect all binding names from a list of statements (top-level idents only).
fn collect_scope_names(stmts: &[Stmt]) -> std::collections::HashSet<Atom> {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct BindingCollector {
        names: std::collections::HashSet<Atom>,
    }
    impl Visit for BindingCollector {
        fn visit_ident(&mut self, id: &Ident) {
            self.names.insert(id.sym.clone());
        }
    }
    let mut collector = BindingCollector {
        names: std::collections::HashSet::new(),
    };
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.names
}

fn collect_scope_names_module(items: &[ModuleItem]) -> std::collections::HashSet<Atom> {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct BindingCollector {
        names: std::collections::HashSet<Atom>,
    }
    impl Visit for BindingCollector {
        fn visit_ident(&mut self, id: &Ident) {
            self.names.insert(id.sym.clone());
        }
    }
    let mut collector = BindingCollector {
        names: std::collections::HashSet::new(),
    };
    for item in items {
        item.visit_with(&mut collector);
    }
    collector.names
}

/// Create a PropName — use Ident for valid JS identifiers, Str for others (e.g. "aria-current").
fn make_prop_name(name: &Atom) -> PropName {
    if is_valid_ident(name) {
        PropName::Ident(swc_core::ecma::ast::IdentName::new(name.clone(), DUMMY_SP))
    } else {
        PropName::Str(swc_core::ecma::ast::Str {
            span: DUMMY_SP,
            value: name.as_str().into(),
            raw: None,
        })
    }
}

/// Check if a string is a valid JS identifier (can be used unquoted as a property name).
fn is_valid_ident(name: &str) -> bool {
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
