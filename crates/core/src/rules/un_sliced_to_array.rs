use std::collections::HashSet;

use crate::facts::{ModuleFactsMap, TypeScriptHelperKind};
use crate::utils::paren::strip_parens;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, AssignExpr, AssignOp, AssignTarget, BinaryOp, BindingIdent, Callee, Decl, Expr,
    ExprStmt, Lit, MemberExpr, MemberProp, Module, ModuleItem, Pat, SimpleAssignTarget, Stmt,
    VarDecl, VarDeclKind, VarDeclarator,
};

use super::cross_module_helper_refs::{
    collect_cross_module_helper_refs, collect_cross_module_ts_helper_refs,
    cross_module_member_helper_kind, CrossModuleHelperRefs,
};
use super::decl_utils::{
    can_remove_prior_uninitialized_decls_by, remove_prior_uninitialized_decls_by,
    UninitializedDeclKind,
};
use super::helper_matcher::BindingKey;
use super::transpiler_helper_utils::{
    collect_maybe_array_like_bindings, extract_inline_sliced_to_array_call,
    ts_expr_matches_helper_kind, tslib_member_ts_helper_kind, tslib_require_ts_helper_kind,
    LocalHelperContext, TranspilerHelperKind, TsHelperKind,
};
use super::RewriteLevel;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

/// Detects and unwraps `_slicedToArray(expr, N)` helper calls.
///
/// Transforms:
///   `var _ref = _slicedToArray(expr, N)` → `var _ref = expr`
///   `var _ref = _slicedToArray(expr, 0)` → `var [] = expr`
///
/// The downstream `SmartInline` + destructuring rules handle converting
/// `var a = _ref[0]; var b = _ref[1]` → `const [a, b] = expr`.
pub struct UnSlicedToArray<'a> {
    module_facts: Option<&'a ModuleFactsMap>,
    level: RewriteLevel,
}

struct SlicedExtraction {
    ref_binding: BindingIdent,
    source: Box<Expr>,
    source_ref: Option<swc_core::ecma::ast::Ident>,
    length: Option<usize>,
}

impl UnSlicedToArray<'_> {
    pub fn new() -> Self {
        Self {
            module_facts: None,
            level: RewriteLevel::Standard,
        }
    }

    pub fn new_with_level(level: RewriteLevel) -> Self {
        Self {
            module_facts: None,
            level,
        }
    }
}

impl<'a> UnSlicedToArray<'a> {
    pub fn new_with_facts(module_facts: &'a ModuleFactsMap) -> Self {
        Self {
            module_facts: Some(module_facts),
            level: RewriteLevel::Standard,
        }
    }

    pub fn new_with_facts_and_level(module_facts: &'a ModuleFactsMap, level: RewriteLevel) -> Self {
        Self {
            module_facts: Some(module_facts),
            level,
        }
    }

    pub(crate) fn run_with_helpers(
        module: &mut Module,
        local_helpers: &LocalHelperContext,
        module_facts: Option<&ModuleFactsMap>,
        level: RewriteLevel,
    ) {
        run_un_sliced_to_array(module, local_helpers, module_facts, level);
    }
}

impl Default for UnSlicedToArray<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl VisitMut for UnSlicedToArray<'_> {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_sliced_to_array(module, &local_helpers, self.module_facts, self.level);
    }
}

fn run_un_sliced_to_array(
    module: &mut Module,
    local_helpers: &LocalHelperContext,
    module_facts: Option<&ModuleFactsMap>,
    level: RewriteLevel,
) {
    let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::SlicedToArray);
    let ts_read_helpers = local_helpers.ts_helpers_of_kind(TsHelperKind::Read);
    let mut cross_module_helpers = module_facts
        .map(|facts| {
            collect_cross_module_helper_refs(module, facts, |kind| {
                kind == TranspilerHelperKind::SlicedToArray
            })
        })
        .unwrap_or_default();
    if let Some(facts) = module_facts {
        extend_cross_module_helpers(
            &mut cross_module_helpers,
            collect_cross_module_ts_helper_refs(module, facts, TypeScriptHelperKind::Read),
            TranspilerHelperKind::SlicedToArray,
        );
    }
    let tslib_namespaces = local_helpers.tslib_namespaces();
    let has_direct_tslib_calls =
        local_helpers.has_tslib_require_member_call(TranspilerHelperKind::SlicedToArray);
    let has_inline_ts_read = has_inline_ts_read_call(module);
    if helpers.is_empty()
        && ts_read_helpers.is_empty()
        && cross_module_helpers.direct.is_empty()
        && cross_module_helpers.namespaces.is_empty()
        && tslib_namespaces.is_empty()
        && !has_direct_tslib_calls
        && !has_inline_ts_read
        && !has_inline_sliced_to_array_candidate(module)
    {
        return;
    }
    let maybe_array_like = collect_maybe_array_like_bindings(module);
    module.visit_mut_children_with(&mut SlicedToArrayRewriter {
        local_helpers,
        cross_module_helpers: &cross_module_helpers,
        maybe_array_like: &maybe_array_like,
        level,
    });

    local_helpers.remove_unused_ts_helper_bindings(module, TsHelperKind::Read);

    if helpers.is_empty() {
        return;
    }

    local_helpers.remove_helpers_with_dependencies(module, helpers);
}

fn has_inline_sliced_to_array_candidate(module: &Module) -> bool {
    struct Scan {
        found: bool,
    }

    impl Visit for Scan {
        fn visit_expr(&mut self, expr: &Expr) {
            if self.found {
                return;
            }
            if extract_inline_sliced_to_array_call(expr).is_some() {
                self.found = true;
                return;
            }
            expr.visit_children_with(self);
        }
    }

    let mut scan = Scan { found: false };
    module.visit_with(&mut scan);
    scan.found
}

struct SlicedToArrayRewriter<'a> {
    local_helpers: &'a LocalHelperContext,
    cross_module_helpers: &'a CrossModuleHelperRefs,
    maybe_array_like: &'a HashSet<BindingKey>,
    level: RewriteLevel,
}

impl VisitMut for SlicedToArrayRewriter<'_> {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        fold_sliced_to_array_module_item_groups(
            items,
            self.local_helpers,
            self.cross_module_helpers,
            self.maybe_array_like,
        );
        for item in items {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                continue;
            };
            rewrite_sliced_to_array_decls(
                &mut var.decls,
                self.local_helpers,
                self.cross_module_helpers,
                self.maybe_array_like,
            );
        }
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        fold_sliced_to_array_stmt_groups(
            stmts,
            self.local_helpers,
            self.cross_module_helpers,
            self.maybe_array_like,
            self.level,
        );
        for stmt in stmts {
            let Stmt::Decl(Decl::Var(var)) = stmt else {
                continue;
            };
            rewrite_sliced_to_array_decls(
                &mut var.decls,
                self.local_helpers,
                self.cross_module_helpers,
                self.maybe_array_like,
            );
        }
    }
}

fn fold_sliced_to_array_module_item_groups(
    body: &mut Vec<ModuleItem>,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
) {
    let mut i = 0;
    while i < body.len() {
        try_fold_sliced_to_array_module_item_group(
            body,
            i,
            local_helpers,
            cross_module_helpers,
            maybe_array_like,
        );
        i += 1;
    }
}

fn try_fold_sliced_to_array_module_item_group(
    body: &mut Vec<ModuleItem>,
    start: usize,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
) -> bool {
    let Some(extraction) = extract_sliced_to_array_module_item(
        &body[start],
        local_helpers,
        cross_module_helpers,
        maybe_array_like,
    ) else {
        return false;
    };
    if extraction.length == Some(0) {
        return false;
    }
    if module_item_sliced_ref_is_unreferenced(body, start, &extraction.ref_binding.id) {
        let Some(length) = extraction.length else {
            return false;
        };
        if sliced_source_ref_is_used_in_items(&body[start + 1..], &extraction) {
            return false;
        }
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &mut body[start] else {
            return false;
        };
        let Some(decl) = var.decls.first_mut() else {
            return false;
        };
        decl.name = Pat::Array(ArrayPat {
            span: DUMMY_SP,
            elems: vec![None; length],
            optional: false,
            type_ann: None,
        });
        decl.init = Some(extraction.source);
        return true;
    }

    let known_length = extraction.length;
    let mut elems = Vec::with_capacity(known_length.unwrap_or(2));
    let max_len = known_length.unwrap_or(64);
    for index in 0..max_len {
        let Some(item) = body.get(start + 1 + index) else {
            break;
        };
        let Some(binding) = extract_ref_index_module_item(item, &extraction.ref_binding.id, index)
        else {
            if known_length.is_some() {
                return false;
            }
            break;
        };
        if body
            .get(start + 2 + index)
            .is_some_and(|item| module_item_is_default_from_temp(item, &binding.id))
        {
            return false;
        }
        elems.push(Some(Pat::Ident(binding)));
    }
    let length = elems.len();
    if length == 0 {
        return false;
    }
    if known_length.is_some_and(|known| known != length) {
        return false;
    }
    if ident_used_in_items(&body[start + 1 + length..], &extraction.ref_binding.id) {
        return false;
    }
    if sliced_source_ref_is_used_in_items(&body[start + 1 + length..], &extraction) {
        return false;
    }

    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &mut body[start] else {
        return false;
    };
    let Some(decl) = var.decls.first_mut() else {
        return false;
    };
    decl.name = Pat::Array(ArrayPat {
        span: DUMMY_SP,
        elems,
        optional: false,
        type_ann: None,
    });
    decl.init = Some(extraction.source);
    body.drain(start + 1..start + 1 + length);
    true
}

fn fold_sliced_to_array_stmt_groups(
    stmts: &mut Vec<Stmt>,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
    level: RewriteLevel,
) {
    let mut i = 0;
    while i < stmts.len() {
        let allow_assignment_index_folds = level >= RewriteLevel::Standard;
        let _ = try_fold_sliced_to_array_stmt_group(
            stmts,
            i,
            local_helpers,
            cross_module_helpers,
            maybe_array_like,
            allow_assignment_index_folds,
        ) || (allow_assignment_index_folds
            && try_fold_sliced_to_array_assignment_stmt_group(
                stmts,
                i,
                local_helpers,
                cross_module_helpers,
                maybe_array_like,
            ))
            || (allow_assignment_index_folds
                && try_fold_sliced_to_array_ref_assignment_stmt_group(
                    stmts,
                    i,
                    local_helpers,
                    cross_module_helpers,
                    maybe_array_like,
                ));
        i += 1;
    }
}

fn try_fold_sliced_to_array_stmt_group(
    stmts: &mut Vec<Stmt>,
    start: usize,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
    allow_assignment_index_folds: bool,
) -> bool {
    let Some(extraction) = extract_sliced_to_array_stmt(
        &stmts[start],
        local_helpers,
        cross_module_helpers,
        maybe_array_like,
    ) else {
        return false;
    };
    if extraction.length == Some(0) {
        return false;
    }
    if allow_assignment_index_folds
        && try_fold_sliced_to_array_stmt_assignment_access_group(stmts, start, extraction)
    {
        return true;
    }
    let Some(extraction) = extract_sliced_to_array_stmt(
        &stmts[start],
        local_helpers,
        cross_module_helpers,
        maybe_array_like,
    ) else {
        return false;
    };
    if stmt_sliced_ref_is_unreferenced(stmts, start, &extraction.ref_binding.id) {
        let Some(length) = extraction.length else {
            return false;
        };
        if sliced_source_ref_is_used_in_stmts(&stmts[start + 1..], &extraction) {
            return false;
        }
        let Stmt::Decl(Decl::Var(var)) = &mut stmts[start] else {
            return false;
        };
        let Some(decl) = var.decls.first_mut() else {
            return false;
        };
        decl.name = Pat::Array(ArrayPat {
            span: DUMMY_SP,
            elems: vec![None; length],
            optional: false,
            type_ann: None,
        });
        decl.init = Some(extraction.source);
        return true;
    }

    let known_length = extraction.length;
    let mut elems = Vec::with_capacity(known_length.unwrap_or(2));
    let max_len = known_length.unwrap_or(64);
    for index in 0..max_len {
        let Some(stmt) = stmts.get(start + 1 + index) else {
            break;
        };
        let Some(binding) = extract_ref_index_stmt(stmt, &extraction.ref_binding.id, index) else {
            if known_length.is_some() {
                return false;
            }
            break;
        };
        if stmts
            .get(start + 2 + index)
            .is_some_and(|stmt| stmt_is_default_from_temp(stmt, &binding.id))
        {
            return false;
        }
        elems.push(Some(Pat::Ident(binding)));
    }
    let length = elems.len();
    if length == 0 {
        return false;
    }
    if known_length.is_some_and(|known| known != length) {
        return false;
    }
    if ident_used_in_stmts(&stmts[start + 1 + length..], &extraction.ref_binding.id) {
        return false;
    }
    if sliced_source_ref_is_used_in_stmts(&stmts[start + 1 + length..], &extraction) {
        return false;
    }

    let Stmt::Decl(Decl::Var(var)) = &mut stmts[start] else {
        return false;
    };
    let Some(decl) = var.decls.first_mut() else {
        return false;
    };
    decl.name = Pat::Array(ArrayPat {
        span: DUMMY_SP,
        elems,
        optional: false,
        type_ann: None,
    });
    decl.init = Some(extraction.source);
    stmts.drain(start + 1..start + 1 + length);
    true
}

fn try_fold_sliced_to_array_stmt_assignment_access_group(
    stmts: &mut Vec<Stmt>,
    start: usize,
    extraction: SlicedExtraction,
) -> bool {
    let Some(length) = extraction.length else {
        return false;
    };
    if length == 0 {
        return false;
    }
    let Some(elems) =
        collect_ref_index_assignment_elems(stmts, start + 1, &extraction.ref_binding.id, length)
    else {
        return false;
    };
    if ident_used_in_stmts(&stmts[start + 1 + length..], &extraction.ref_binding.id) {
        return false;
    }
    if sliced_source_ref_is_used_in_stmts(&stmts[start + 1 + length..], &extraction) {
        return false;
    }
    let targets: Vec<_> = elems
        .iter()
        .filter_map(|elem| match elem {
            Some(Pat::Ident(binding)) => Some(binding.id.clone()),
            _ => None,
        })
        .collect();
    if !can_remove_prior_uninitialized_decls_by(
        &stmts[..start],
        &targets,
        UninitializedDeclKind::VarOnly,
        same_sliced_ref_ident,
    ) {
        return false;
    }

    let Stmt::Decl(Decl::Var(var)) = &mut stmts[start] else {
        return false;
    };
    let Some(decl) = var.decls.first_mut() else {
        return false;
    };
    decl.name = Pat::Array(ArrayPat {
        span: DUMMY_SP,
        elems,
        optional: false,
        type_ann: None,
    });
    decl.init = Some(extraction.source);
    stmts.drain(start + 1..start + 1 + length);
    remove_prior_uninitialized_decls_by(
        stmts,
        start,
        &targets,
        UninitializedDeclKind::VarOnly,
        same_sliced_ref_ident,
    );
    true
}

fn try_fold_sliced_to_array_assignment_stmt_group(
    stmts: &mut Vec<Stmt>,
    start: usize,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
) -> bool {
    let Some((first, extraction)) = extract_sliced_to_array_assignment_stmt(
        &stmts[start],
        local_helpers,
        cross_module_helpers,
        maybe_array_like,
    ) else {
        return false;
    };
    if extraction.length != Some(2) {
        return false;
    }
    let Some(second_stmt) = stmts.get(start + 1) else {
        return false;
    };
    let Some(second) =
        extract_ref_index_assignment_stmt(second_stmt, &extraction.ref_binding.id, 1)
    else {
        return false;
    };
    if ident_used_in_stmts(&stmts[start + 2..], &extraction.ref_binding.id) {
        return false;
    }
    if sliced_source_ref_is_used_in_stmts(&stmts[start + 2..], &extraction) {
        return false;
    }
    let ref_id = extraction.ref_binding.id.clone();
    let first_id = first.id.clone();
    let second_id = second.id.clone();
    let targets = vec![ref_id, first_id, second_id];
    if !can_remove_prior_uninitialized_decls_by(
        &stmts[..start],
        &targets,
        UninitializedDeclKind::VarOnly,
        same_sliced_ref_ident,
    ) {
        return false;
    }

    stmts[start] = Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Array(ArrayPat {
                span: DUMMY_SP,
                elems: vec![Some(Pat::Ident(first)), Some(Pat::Ident(second))],
                optional: false,
                type_ann: None,
            }),
            init: Some(extraction.source),
            definite: false,
        }],
    })));
    stmts.drain(start + 1..start + 2);
    remove_prior_uninitialized_decls_by(
        stmts,
        start,
        &targets,
        UninitializedDeclKind::VarOnly,
        same_sliced_ref_ident,
    );
    true
}

fn try_fold_sliced_to_array_ref_assignment_stmt_group(
    stmts: &mut Vec<Stmt>,
    start: usize,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
) -> bool {
    let Some(extraction) = extract_sliced_to_array_ref_assignment_stmt(
        &stmts[start],
        local_helpers,
        cross_module_helpers,
        maybe_array_like,
    ) else {
        return false;
    };
    let Some(length) = extraction.length else {
        return false;
    };
    if length == 0 {
        return false;
    }
    let Some(elems) =
        collect_ref_index_assignment_elems(stmts, start + 1, &extraction.ref_binding.id, length)
    else {
        return false;
    };
    if ident_used_in_stmts(&stmts[start + 1 + length..], &extraction.ref_binding.id) {
        return false;
    }
    if sliced_source_ref_is_used_in_stmts(&stmts[start + 1 + length..], &extraction) {
        return false;
    }

    let mut targets = vec![extraction.ref_binding.id.clone()];
    targets.extend(elems.iter().filter_map(|elem| match elem {
        Some(Pat::Ident(binding)) => Some(binding.id.clone()),
        _ => None,
    }));
    if !can_remove_prior_uninitialized_decls_by(
        &stmts[..start],
        &targets,
        UninitializedDeclKind::VarOnly,
        same_sliced_ref_ident,
    ) {
        return false;
    }

    stmts[start] = Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Array(ArrayPat {
                span: DUMMY_SP,
                elems,
                optional: false,
                type_ann: None,
            }),
            init: Some(extraction.source),
            definite: false,
        }],
    })));
    stmts.drain(start + 1..start + 1 + length);
    remove_prior_uninitialized_decls_by(
        stmts,
        start,
        &targets,
        UninitializedDeclKind::VarOnly,
        same_sliced_ref_ident,
    );
    true
}

fn extract_sliced_to_array_module_item(
    item: &ModuleItem,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
) -> Option<SlicedExtraction> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    extract_sliced_to_array_decl(
        &var.decls[0],
        local_helpers,
        cross_module_helpers,
        maybe_array_like,
    )
}

fn extract_sliced_to_array_stmt(
    stmt: &Stmt,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
) -> Option<SlicedExtraction> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    extract_sliced_to_array_decl(
        &var.decls[0],
        local_helpers,
        cross_module_helpers,
        maybe_array_like,
    )
}

fn extract_sliced_to_array_assignment_stmt(
    stmt: &Stmt,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
) -> Option<(BindingIdent, SlicedExtraction)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(expr.as_ref()) else {
        return None;
    };
    let first = simple_ident_assign_target(assign)?;
    let Expr::Member(member) = strip_parens(assign.right.as_ref()) else {
        return None;
    };
    if member_index(member)? != 0 {
        return None;
    }
    let Expr::Assign(tuple_assign) = strip_parens(member.obj.as_ref()) else {
        return None;
    };
    let tuple = simple_ident_assign_target(tuple_assign)?;
    let Expr::Call(call) = strip_parens(tuple_assign.right.as_ref()) else {
        return None;
    };

    let (source, length_val) =
        extract_sliced_call_args(call, local_helpers, cross_module_helpers, maybe_array_like)?;
    let length = numeric_length(length_val)?;

    Some((
        BindingIdent {
            id: first,
            type_ann: None,
        },
        SlicedExtraction {
            ref_binding: BindingIdent {
                id: tuple,
                type_ann: None,
            },
            source: Box::new(source.clone()),
            source_ref: None,
            length: Some(length),
        },
    ))
}

fn extract_sliced_to_array_ref_assignment_stmt(
    stmt: &Stmt,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
) -> Option<SlicedExtraction> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(expr.as_ref()) else {
        return None;
    };
    let tuple = simple_ident_assign_target(assign)?;
    let Expr::Call(call) = strip_parens(assign.right.as_ref()) else {
        return None;
    };

    let (source, length_val) =
        extract_sliced_call_args(call, local_helpers, cross_module_helpers, maybe_array_like)?;
    let length = numeric_length(length_val)?;

    Some(SlicedExtraction {
        ref_binding: BindingIdent {
            id: tuple,
            type_ann: None,
        },
        source: Box::new(source.clone()),
        source_ref: None,
        length: Some(length),
    })
}

fn extract_ref_index_assignment_stmt(
    stmt: &Stmt,
    ref_ident: &swc_core::ecma::ast::Ident,
    index: usize,
) -> Option<BindingIdent> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(expr.as_ref()) else {
        return None;
    };
    let target = simple_ident_assign_target(assign)?;
    let Expr::Member(member) = strip_parens(assign.right.as_ref()) else {
        return None;
    };
    let Expr::Ident(obj) = strip_parens(member.obj.as_ref()) else {
        return None;
    };
    if !same_sliced_ref_ident(obj, ref_ident) || member_index(member)? != index {
        return None;
    }
    Some(BindingIdent {
        id: target,
        type_ann: None,
    })
}

fn collect_ref_index_assignment_elems(
    stmts: &[Stmt],
    start: usize,
    ref_ident: &swc_core::ecma::ast::Ident,
    length: usize,
) -> Option<Vec<Option<Pat>>> {
    let mut elems = Vec::with_capacity(length);
    for index in 0..length {
        let binding =
            extract_ref_index_assignment_stmt(stmts.get(start + index)?, ref_ident, index)?;
        elems.push(Some(Pat::Ident(binding)));
    }
    Some(elems)
}

fn extract_ref_index_module_item(
    item: &ModuleItem,
    ref_ident: &swc_core::ecma::ast::Ident,
    index: usize,
) -> Option<BindingIdent> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    extract_ref_index_binding(&var.decls[0], ref_ident, index)
}

fn extract_ref_index_stmt(
    stmt: &Stmt,
    ref_ident: &swc_core::ecma::ast::Ident,
    index: usize,
) -> Option<BindingIdent> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    extract_ref_index_binding(&var.decls[0], ref_ident, index)
}

fn module_item_is_default_from_temp(item: &ModuleItem, temp: &swc_core::ecma::ast::Ident) -> bool {
    let ModuleItem::Stmt(stmt) = item else {
        return false;
    };
    stmt_is_default_from_temp(stmt, temp)
}

fn stmt_is_default_from_temp(stmt: &Stmt, temp: &swc_core::ecma::ast::Ident) -> bool {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return false;
    };
    if var.decls.len() != 1 {
        return false;
    }
    let Some(init) = &var.decls[0].init else {
        return false;
    };
    expr_is_default_from_temp(init, temp)
}

fn expr_is_default_from_temp(expr: &Expr, temp: &swc_core::ecma::ast::Ident) -> bool {
    let Expr::Cond(cond) = expr else {
        return false;
    };
    if !expr_is_equality_check_for_temp(cond.test.as_ref(), temp) {
        return false;
    }
    expr_is_temp(cond.alt.as_ref(), temp)
}

fn expr_is_equality_check_for_temp(expr: &Expr, temp: &swc_core::ecma::ast::Ident) -> bool {
    let Expr::Bin(bin) = expr else {
        return false;
    };
    if !matches!(bin.op, BinaryOp::EqEqEq | BinaryOp::EqEq) {
        return false;
    }
    expr_is_temp(bin.left.as_ref(), temp) || expr_is_temp(bin.right.as_ref(), temp)
}

fn expr_is_temp(expr: &Expr, temp: &swc_core::ecma::ast::Ident) -> bool {
    matches!(expr, Expr::Ident(id) if same_sliced_ref_ident(id, temp))
}

fn module_item_sliced_ref_is_unreferenced(
    body: &[ModuleItem],
    start: usize,
    ref_ident: &swc_core::ecma::ast::Ident,
) -> bool {
    body.get(start + 1)
        .is_none_or(|_| !ident_used_in_items(&body[start + 1..], ref_ident))
}

fn stmt_sliced_ref_is_unreferenced(
    stmts: &[Stmt],
    start: usize,
    ref_ident: &swc_core::ecma::ast::Ident,
) -> bool {
    stmts
        .get(start + 1)
        .is_none_or(|_| !ident_used_in_stmts(&stmts[start + 1..], ref_ident))
}

fn rewrite_sliced_to_array_decls(
    decls: &mut Vec<VarDeclarator>,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
) {
    let mut i = 0;
    while i < decls.len() {
        try_unwrap_sliced_to_array(
            &mut decls[i],
            local_helpers,
            cross_module_helpers,
            maybe_array_like,
        );
        i += 1;
    }
}

fn extract_sliced_to_array_decl(
    decl: &VarDeclarator,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
) -> Option<SlicedExtraction> {
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let Expr::Call(call) = init.as_ref() else {
        return extract_inline_sliced_to_array_call(init.as_ref()).map(|inline_call| {
            SlicedExtraction {
                ref_binding: binding.clone(),
                source: inline_call.source,
                source_ref: inline_call.source_ref,
                length: inline_call.length,
            }
        });
    };

    let (source, length_val) =
        extract_sliced_call_args(call, local_helpers, cross_module_helpers, maybe_array_like)?;
    let length = numeric_length(length_val)?;
    Some(SlicedExtraction {
        ref_binding: binding.clone(),
        source: Box::new(source.clone()),
        source_ref: None,
        length: Some(length),
    })
}

fn sliced_source_ref_is_used_in_items(items: &[ModuleItem], extraction: &SlicedExtraction) -> bool {
    extraction
        .source_ref
        .as_ref()
        .is_some_and(|source_ref| ident_used_in_items(items, source_ref))
}

fn sliced_source_ref_is_used_in_stmts(stmts: &[Stmt], extraction: &SlicedExtraction) -> bool {
    extraction
        .source_ref
        .as_ref()
        .is_some_and(|source_ref| ident_used_in_stmts(stmts, source_ref))
}

fn extract_ref_index_binding(
    decl: &VarDeclarator,
    ref_ident: &swc_core::ecma::ast::Ident,
    index: usize,
) -> Option<BindingIdent> {
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let Expr::Member(member) = init.as_ref() else {
        return None;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return None;
    };
    if !same_sliced_ref_ident(obj, ref_ident) {
        return None;
    }
    let MemberProp::Computed(computed) = &member.prop else {
        return None;
    };
    let Expr::Lit(Lit::Num(num)) = computed.expr.as_ref() else {
        return None;
    };
    (numeric_length(num.value)? == index).then(|| binding.clone())
}

fn simple_ident_assign_target(assign: &AssignExpr) -> Option<swc_core::ecma::ast::Ident> {
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) = &assign.left else {
        return None;
    };
    Some(ident.id.clone())
}

fn member_index(member: &MemberExpr) -> Option<usize> {
    let MemberProp::Computed(computed) = &member.prop else {
        return None;
    };
    let Expr::Lit(Lit::Num(num)) = strip_parens(computed.expr.as_ref()) else {
        return None;
    };
    numeric_length(num.value)
}

fn try_unwrap_sliced_to_array(
    decl: &mut VarDeclarator,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
) {
    let Some(init) = &decl.init else { return };
    let Expr::Call(call) = init.as_ref() else {
        return;
    };

    let Some((source, length_val)) =
        extract_sliced_call_args(call, local_helpers, cross_module_helpers, maybe_array_like)
    else {
        return;
    };
    let Some(length) = numeric_length(length_val) else {
        return;
    };

    if length == 0 {
        decl.name = Pat::Array(ArrayPat {
            span: DUMMY_SP,
            elems: vec![],
            optional: false,
            type_ann: None,
        });
    }
    decl.init = Some(Box::new(source.clone()));
}

fn is_sliced_to_array_callee(
    callee: &Expr,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
) -> bool {
    local_helpers.is_helper_callee(callee, TranspilerHelperKind::SlicedToArray)
        || matches!(
            callee,
            Expr::Ident(id)
                if local_helpers
                    .ts_helpers_of_kind(TsHelperKind::Read)
                    .contains(&(id.sym.clone(), id.ctxt))
        )
        || matches!(
            callee,
            Expr::Ident(id)
                if cross_module_helpers
                    .direct
                    .contains_key(&(id.sym.clone(), id.ctxt))
        )
        || cross_module_member_helper_kind(callee, &cross_module_helpers.namespaces)
            == Some(TranspilerHelperKind::SlicedToArray)
        || ts_expr_matches_helper_kind(callee, TsHelperKind::Read)
        || tslib_member_ts_helper_kind(callee, local_helpers.tslib_namespaces())
            == Some(TsHelperKind::Read)
        || tslib_require_ts_helper_kind(callee) == Some(TsHelperKind::Read)
}

fn extend_cross_module_helpers(
    helpers: &mut CrossModuleHelperRefs,
    extra: super::cross_module_helper_refs::CrossModuleTsHelperRefs,
    kind: TranspilerHelperKind,
) {
    helpers
        .direct
        .extend(extra.direct.into_iter().map(|key| (key, kind)));
    for (namespace, members) in extra.namespaces {
        helpers
            .namespaces
            .entry(namespace)
            .or_default()
            .extend(members.into_iter().map(|name| (name, kind)));
    }
}

fn has_inline_ts_read_call(module: &Module) -> bool {
    struct Finder {
        found: bool,
    }

    impl Visit for Finder {
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if self.found {
                return;
            }
            let Callee::Expr(callee) = &call.callee else {
                return;
            };
            if ts_expr_matches_helper_kind(callee, TsHelperKind::Read) {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder { found: false };
    module.visit_with(&mut finder);
    finder.found
}

/// Extract `(source, length)` from either `_slicedToArray(src, n)` or
/// `_maybeArrayLike(_slicedToArray, src, n)`.
fn extract_sliced_call_args<'a>(
    call: &'a swc_core::ecma::ast::CallExpr,
    local_helpers: &LocalHelperContext,
    cross_module_helpers: &CrossModuleHelperRefs,
    maybe_array_like: &HashSet<BindingKey>,
) -> Option<(&'a Expr, f64)> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };

    if is_sliced_to_array_callee(callee, local_helpers, cross_module_helpers)
        && call.args.len() == 2
    {
        let Expr::Lit(Lit::Num(num)) = call.args[1].expr.as_ref() else {
            return None;
        };
        return Some((call.args[0].expr.as_ref(), num.value));
    }

    if call.args.len() == 3
        && is_maybe_array_like_callee(callee, maybe_array_like)
        && is_sliced_to_array_callee(
            call.args[0].expr.as_ref(),
            local_helpers,
            cross_module_helpers,
        )
    {
        let Expr::Lit(Lit::Num(num)) = call.args[2].expr.as_ref() else {
            return None;
        };
        return Some((call.args[1].expr.as_ref(), num.value));
    }

    None
}

fn is_maybe_array_like_callee(callee: &Expr, maybe_array_like: &HashSet<BindingKey>) -> bool {
    let Expr::Ident(id) = callee else {
        return false;
    };
    maybe_array_like.contains(&(id.sym.clone(), id.ctxt))
}

fn numeric_length(value: f64) -> Option<usize> {
    if value < 0.0 || value.fract() != 0.0 || value > 64.0 {
        return None;
    }
    Some(value as usize)
}

fn same_sliced_ref_ident(
    obj: &swc_core::ecma::ast::Ident,
    ref_ident: &swc_core::ecma::ast::Ident,
) -> bool {
    obj.sym == ref_ident.sym
        && (obj.ctxt == ref_ident.ctxt
            || (obj.ctxt == SyntaxContext::empty() && ref_ident.ctxt != SyntaxContext::empty()))
}

fn ident_used_in_items(items: &[ModuleItem], target: &swc_core::ecma::ast::Ident) -> bool {
    let mut finder = IdentUseFinder {
        target,
        found: false,
    };
    for item in items {
        item.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

fn ident_used_in_stmts(stmts: &[Stmt], target: &swc_core::ecma::ast::Ident) -> bool {
    let mut finder = IdentUseFinder {
        target,
        found: false,
    };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

struct IdentUseFinder<'a> {
    target: &'a swc_core::ecma::ast::Ident,
    found: bool,
}

impl Visit for IdentUseFinder<'_> {
    fn visit_binding_ident(&mut self, _: &BindingIdent) {}

    fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
        if same_sliced_ref_ident(ident, self.target) {
            self.found = true;
        }
    }
}
