use swc_core::atoms::Atom;
use swc_core::common::{Mark, Span, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, AssignExpr, AssignOp, AssignPat, AssignPatProp, AssignTarget, AssignTargetPat,
    BinaryOp, BindingIdent, BlockStmt, Bool, Callee, CondExpr, Decl, Expr, ExprOrSpread, ExprStmt,
    Function, Ident, IdentName, KeyValuePatProp, Lit, MemberExpr, MemberProp, Module, ModuleItem,
    Number, ObjectPat, ObjectPatProp, Param, Pat, PropName, RestPat, SimpleAssignTarget, Stmt,
    VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::decl_utils::{
    can_remove_prior_uninitialized_decls_by, remove_prior_uninitialized_decls_by,
    UninitializedDeclKind,
};
use super::helper_matcher::{binding_key, BindingKey};
use super::{expr_utils::is_unresolved_undefined, RewriteLevel};
use crate::utils::paren::strip_parens;

/// Reconstructs destructuring from compiler-lowered ref/temp declarations.
///
/// This rule intentionally targets the shape emitted by transforms like SWC's
/// es2015 destructuring pass, rather than guessing from arbitrary property
/// reads. `SmartInline` remains the later readability heuristic for simpler
/// adjacent accesses.
pub struct UnDestructuring {
    unresolved_mark: Mark,
    level: RewriteLevel,
}

impl UnDestructuring {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self::new_with_level(unresolved_mark, RewriteLevel::Standard)
    }

    pub fn new_with_level(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
        }
    }
}

#[derive(Clone)]
struct RefDecl {
    span: swc_core::common::Span,
    ctxt: swc_core::common::SyntaxContext,
    kind: VarDeclKind,
    declare: bool,
    ident: BindingIdent,
    init: Box<Expr>,
}

#[derive(Clone)]
enum Access {
    Array { index: usize, pat: Pat },
    ArrayRest { start: usize, binding: BindingIdent },
    Object { key: PropKey, pat: Pat },
}

#[derive(Clone)]
enum SourceAccess {
    ArrayIndex(usize),
    ObjectProp(PropKey),
}

#[derive(Clone)]
enum PropKey {
    Ident(Atom),
    Str(Atom),
}

impl VisitMut for UnDestructuring {
    fn visit_mut_module(&mut self, module: &mut Module) {
        module.visit_mut_children_with(self);
        module.body = process_module_items(
            std::mem::take(&mut module.body),
            self.unresolved_mark,
            self.level,
        );
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        *stmts = process_stmts(std::mem::take(stmts), self.unresolved_mark, self.level);
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        func.visit_mut_children_with(self);
        if self.level >= RewriteLevel::Standard {
            if let Some(body) = &mut func.body {
                nest_param_destructuring(&mut func.params, body, self.unresolved_mark);
            }
        }
    }
}

fn process_module_items(
    items: Vec<ModuleItem>,
    unresolved_mark: Mark,
    level: RewriteLevel,
) -> Vec<ModuleItem> {
    let mut result = Vec::with_capacity(items.len());
    let mut stmt_buf = Vec::new();

    for item in items {
        match item {
            ModuleItem::Stmt(stmt) => stmt_buf.push(stmt),
            other => {
                if !stmt_buf.is_empty() {
                    result.extend(
                        process_stmts(std::mem::take(&mut stmt_buf), unresolved_mark, level)
                            .into_iter()
                            .map(ModuleItem::Stmt),
                    );
                }
                result.push(other);
            }
        }
    }

    if !stmt_buf.is_empty() {
        result.extend(
            process_stmts(stmt_buf, unresolved_mark, level)
                .into_iter()
                .map(ModuleItem::Stmt),
        );
    }

    result
}

fn process_stmts(stmts: Vec<Stmt>, unresolved_mark: Mark, level: RewriteLevel) -> Vec<Stmt> {
    let mut stmts = hoist_conditional_test_assignments(stmts);
    let mut result = Vec::with_capacity(stmts.len());
    let mut consumed_helpers: Vec<BindingKey> = Vec::new();
    let mut i = 0;

    while i < stmts.len() {
        if let Some(group) =
            try_reconstruct_group(&stmts, i, unresolved_mark, level, &mut consumed_helpers)
        {
            remove_prior_uninitialized_decls_for_bindings(
                &mut result,
                &group.remove_prior_bindings,
            );
            result.push(group.stmt);
            i += group.consumed;
        } else {
            result.push(std::mem::replace(
                &mut stmts[i],
                Stmt::Empty(swc_core::ecma::ast::EmptyStmt {
                    span: swc_core::common::DUMMY_SP,
                }),
            ));
            i += 1;
        }
    }

    if !consumed_helpers.is_empty() {
        remove_unreferenced_helpers(&mut result, &consumed_helpers);
    }

    result
}

struct ReconstructedGroup {
    stmt: Stmt,
    consumed: usize,
    remove_prior_bindings: Vec<BindingKey>,
}

fn remove_prior_uninitialized_decls_for_bindings(stmts: &mut Vec<Stmt>, bindings: &[BindingKey]) {
    let removable: Vec<_> = bindings
        .iter()
        .map(|(sym, ctxt)| Ident::new(sym.clone(), DUMMY_SP, *ctxt))
        .filter(|target| {
            can_remove_prior_uninitialized_decls_by(
                stmts,
                std::slice::from_ref(target),
                UninitializedDeclKind::Any,
                same_binding_ident,
            )
        })
        .collect();

    if removable.is_empty() {
        return;
    }

    remove_prior_uninitialized_decls_by(
        stmts,
        stmts.len(),
        &removable,
        UninitializedDeclKind::Any,
        same_binding_ident,
    );
}

fn same_binding_ident(left: &Ident, right: &Ident) -> bool {
    left.sym == right.sym && left.ctxt == right.ctxt
}

/// Un-fuses minifier output that inlined an array/object extraction into the
/// test of a conditional, e.g. `_f = (backup = _e[2]) != null ? backup : y`.
/// The embedded `backup = _e[2]` assignment is hoisted to its own preceding
/// statement so the surrounding destructuring group can pick it up. Restricted
/// to member-access right-hand sides so it only targets the extraction pattern.
fn hoist_conditional_test_assignments(stmts: Vec<Stmt>) -> Vec<Stmt> {
    let mut result = Vec::with_capacity(stmts.len());
    for mut stmt in stmts {
        if let Some(hoisted) = take_hoistable_cond_test_assignment(&mut stmt) {
            result.push(hoisted);
        }
        result.push(stmt);
    }
    result
}

fn take_hoistable_cond_test_assignment(stmt: &mut Stmt) -> Option<Stmt> {
    let (outer_target, cond) = cond_value_of_stmt_mut(stmt)?;
    let Expr::Bin(bin) = cond.test.as_mut() else {
        return None;
    };
    if !matches!(
        bin.op,
        BinaryOp::EqEq | BinaryOp::NotEq | BinaryOp::EqEqEq | BinaryOp::NotEqEq
    ) {
        return None;
    }
    if let Some(hoisted) = take_member_assign_operand(&mut bin.left, &outer_target) {
        return Some(hoisted);
    }
    take_member_assign_operand(&mut bin.right, &outer_target)
}

/// Returns the statement's outer assignment target (if any) and a mutable
/// reference to a conditional it assigns/initializes.
fn cond_value_of_stmt_mut(stmt: &mut Stmt) -> Option<(Option<BindingKey>, &mut CondExpr)> {
    match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => match expr.as_mut() {
            Expr::Assign(assign) if assign.op == AssignOp::Assign => {
                let target = match &assign.left {
                    AssignTarget::Simple(SimpleAssignTarget::Ident(id)) => {
                        Some(binding_key(&id.id))
                    }
                    _ => None,
                };
                match assign.right.as_mut() {
                    Expr::Cond(cond) => Some((target, cond)),
                    _ => None,
                }
            }
            Expr::Cond(cond) => Some((None, cond)),
            _ => None,
        },
        Stmt::Decl(Decl::Var(var)) if var.decls.len() == 1 => {
            let target = match &var.decls[0].name {
                Pat::Ident(id) => Some(binding_key(&id.id)),
                _ => None,
            };
            match var.decls[0].init.as_deref_mut()? {
                Expr::Cond(cond) => Some((target, cond)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// If `operand` is `ident = <member-access>`, replace it with `ident` and
/// return the hoisted `ident = <member-access>;` statement. Skips the minifier
/// self-assign idiom (`o = (o = x.y) != null ? ...`) where the inner target
/// matches the outer one — there is no destructuring element to recover there.
fn take_member_assign_operand(
    operand: &mut Box<Expr>,
    outer_target: &Option<BindingKey>,
) -> Option<Stmt> {
    let Expr::Assign(assign) = strip_parens(operand.as_ref()) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) = &assign.left else {
        return None;
    };
    // Only an extraction from a destructuring source: `ident = <obj>.<m>` where
    // `<obj>` is a binding (`Z`) or an inline-established one (`(V = …)`).
    // Excludes `ident = (a ?? {}).p` / `ident = a.b.c` where no destructuring
    // group can form, which would split statements for no benefit.
    let Expr::Member(member) = strip_parens(assign.right.as_ref()) else {
        return None;
    };
    if !matches!(
        strip_parens(member.obj.as_ref()),
        Expr::Ident(_) | Expr::Assign(_)
    ) {
        return None;
    }
    if outer_target
        .as_ref()
        .is_some_and(|target| *target == binding_key(&ident.id))
    {
        return None;
    }
    let hoisted = Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Assign(assign.clone())),
    });
    **operand = Expr::Ident(ident.id.clone());
    Some(hoisted)
}

fn try_reconstruct_group(
    stmts: &[Stmt],
    start: usize,
    unresolved_mark: Mark,
    level: RewriteLevel,
    consumed_helpers: &mut Vec<BindingKey>,
) -> Option<ReconstructedGroup> {
    try_reconstruct_assignment_group(stmts, start, unresolved_mark)
        .or_else(|| try_reconstruct_ref_group(stmts, start, unresolved_mark, consumed_helpers))
        .or_else(|| {
            (level >= RewriteLevel::Aggressive)
                .then(|| try_reconstruct_direct_array_group(stmts, start, unresolved_mark))
                .flatten()
        })
}

fn try_reconstruct_assignment_group(
    stmts: &[Stmt],
    start: usize,
    unresolved_mark: Mark,
) -> Option<ReconstructedGroup> {
    let first = try_extract_assignment_access(stmts, start, None, unresolved_mark)?;
    let source = first.source;
    let init = first.init;
    let mut accesses = vec![first.access];
    let mut removed_temps = first.removed_temps;

    let mut i = start + first.consumed;
    while i < stmts.len() {
        if let Some(next) = try_extract_assignment_access(stmts, i, Some(&source), unresolved_mark)
        {
            accesses.push(next.access);
            removed_temps.extend(next.removed_temps);
            i += next.consumed;
        } else {
            break;
        }
    }

    if accesses.is_empty() || !accesses.iter().any(is_rest_or_default_access) {
        return None;
    }

    if accesses
        .iter()
        .any(|access| default_uses_any_removed_binding(access, &removed_temps))
    {
        return None;
    }

    for temp in &removed_temps {
        if ident_used_in_stmts(&stmts[i..], temp) {
            return None;
        }
    }

    let stmt = build_assignment_destructuring_stmt(accesses, init)?;
    Some(ReconstructedGroup {
        stmt,
        consumed: i - start,
        remove_prior_bindings: removed_temps,
    })
}

struct AssignmentAccess {
    source: Ident,
    init: Box<Expr>,
    access: Access,
    consumed: usize,
    removed_temps: Vec<BindingKey>,
}

fn try_extract_assignment_access(
    stmts: &[Stmt],
    index: usize,
    expected_source: Option<&Ident>,
    unresolved_mark: Mark,
) -> Option<AssignmentAccess> {
    if let Some(extracted) =
        try_extract_assignment_sliced_default_access(stmts, index, expected_source, unresolved_mark)
    {
        return Some(extracted);
    }

    if let Some(extracted) =
        try_extract_assignment_member_default_access(stmts, index, expected_source, unresolved_mark)
    {
        return Some(extracted);
    }

    if let Some(extracted) =
        try_extract_assignment_fused_default_access(stmts, index, expected_source, unresolved_mark)
    {
        return Some(extracted);
    }

    if let Some(mut extracted) =
        try_extract_assignment_default_access(stmts, index, expected_source, unresolved_mark)
    {
        if let Some((nested_pat, extra)) = try_nest_assignment_default_binding(
            &extracted.access,
            stmts,
            index + extracted.consumed,
            unresolved_mark,
            &mut extracted.removed_temps,
        ) {
            replace_access_left(&mut extracted.access, nested_pat);
            extracted.consumed += extra;
        }
        return Some(extracted);
    }

    let (binding, init) = extract_binding_assignment(stmts.get(index)?)?;
    let (source, source_init, source_access) =
        extract_assignment_source_access(init, expected_source)?;
    let access = match source_access {
        SourceAccess::ArrayIndex(index) => Access::Array {
            index,
            pat: Pat::Ident(binding),
        },
        SourceAccess::ObjectProp(key) => Access::Object {
            key,
            pat: Pat::Ident(binding),
        },
    };

    Some(AssignmentAccess {
        source,
        init: source_init,
        access,
        consumed: 1,
        removed_temps: Vec::new(),
    })
}

fn try_extract_assignment_member_default_access(
    stmts: &[Stmt],
    index: usize,
    expected_source: Option<&Ident>,
    unresolved_mark: Mark,
) -> Option<AssignmentAccess> {
    let (temp, temp_init) = extract_binding_assignment(stmts.get(index)?)?;
    let (source, source_init, source_access) =
        extract_assignment_source_access(temp_init, expected_source)?;

    let (binding, binding_init) = extract_binding_assignment(stmts.get(index + 1)?)?;
    let (default, nested_source_access) =
        extract_default_member_access(binding_init, &temp.id, unresolved_mark)?;
    let temp_key = binding_key(&temp.id);
    if expr_uses_ident(&default, &temp_key) {
        return None;
    }

    let nested_access = match nested_source_access {
        SourceAccess::ArrayIndex(index) => Access::Array {
            index,
            pat: Pat::Ident(binding),
        },
        SourceAccess::ObjectProp(key) => Access::Object {
            key,
            pat: Pat::Ident(binding),
        },
    };
    let nested_pat = build_pat_from_accesses(vec![nested_access])?;
    let pat = Pat::Assign(AssignPat {
        span: DUMMY_SP,
        left: Box::new(nested_pat),
        right: default,
    });

    let access = match source_access {
        SourceAccess::ArrayIndex(index) => Access::Array { index, pat },
        SourceAccess::ObjectProp(key) => Access::Object { key, pat },
    };

    Some(AssignmentAccess {
        source,
        init: source_init,
        access,
        consumed: 2,
        removed_temps: vec![temp_key],
    })
}

fn try_extract_assignment_sliced_default_access(
    stmts: &[Stmt],
    index: usize,
    expected_source: Option<&Ident>,
    unresolved_mark: Mark,
) -> Option<AssignmentAccess> {
    let (temp, temp_init) = extract_binding_assignment(stmts.get(index)?)?;
    let (source, source_init, source_access) =
        extract_assignment_source_access(temp_init, expected_source)?;

    let (ref_binding, ref_init) = extract_binding_assignment(stmts.get(index + 1)?)?;
    let Expr::Call(call) = strip_parens(ref_init) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !is_sliced_to_array_callee_name(callee.as_ref()) || call.args.len() != 2 {
        return None;
    }
    let (default, fused_temp) =
        extract_sliced_default_arg(call.args[0].expr.as_ref(), &temp.id, unresolved_mark)?;

    let temp_key = binding_key(&temp.id);
    let ref_key = binding_key(&ref_binding.id);
    if expr_uses_ident(&default, &temp_key) {
        return None;
    }

    let mut removed_temps = vec![temp_key, ref_key];
    removed_temps.extend(fused_temp.map(|id| binding_key(&id)));
    let collected = collect_assignment_accesses_on(
        stmts,
        index + 2,
        &ref_binding.id,
        unresolved_mark,
        &mut removed_temps,
    );
    if collected.accesses.is_empty() {
        return None;
    }

    let nested_pat = build_pat_from_accesses(collected.accesses)?;
    let pat = Pat::Assign(AssignPat {
        span: DUMMY_SP,
        left: Box::new(nested_pat),
        right: default,
    });
    let access = match source_access {
        SourceAccess::ArrayIndex(index) => Access::Array { index, pat },
        SourceAccess::ObjectProp(key) => Access::Object { key, pat },
    };

    Some(AssignmentAccess {
        source,
        init: source_init,
        access,
        consumed: 2 + collected.consumed,
        removed_temps,
    })
}

/// Handles the minifier-fused form where the defaulted temp is assigned inline
/// inside the first sub-access:
/// `b = src.key; first = (ref = b === undefined ? DEFAULT : b).<m>; … = ref[…]`
/// recovering `key: <nested-pat> = DEFAULT`. Covers both nested object members
/// and array indices (with holes) on the shared `ref` binding.
fn try_extract_assignment_fused_default_access(
    stmts: &[Stmt],
    index: usize,
    expected_source: Option<&Ident>,
    unresolved_mark: Mark,
) -> Option<AssignmentAccess> {
    let (temp, temp_init) = extract_binding_assignment(stmts.get(index)?)?;
    let (source, source_init, source_access) =
        extract_assignment_source_access(temp_init, expected_source)?;

    // `first = (ref = temp === undefined ? DEFAULT : temp).<member|[idx]>`
    let (first_binding, first_init) = extract_binding_assignment(stmts.get(index + 1)?)?;
    let Expr::Member(member) = strip_parens(first_init) else {
        return None;
    };
    let Expr::Assign(ref_assign) = strip_parens(member.obj.as_ref()) else {
        return None;
    };
    if ref_assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(ref_binding)) = &ref_assign.left else {
        return None;
    };
    let default = extract_default_value(
        strip_parens(ref_assign.right.as_ref()),
        &temp.id,
        unresolved_mark,
    )?;
    let temp_key = binding_key(&temp.id);
    if expr_uses_ident(&default, &temp_key) {
        return None;
    }
    let ref_key = binding_key(&ref_binding.id);

    let first_access = match source_access_from_member_prop(&member.prop)? {
        SourceAccess::ObjectProp(key) => Access::Object {
            key,
            pat: Pat::Ident(first_binding),
        },
        SourceAccess::ArrayIndex(idx) => Access::Array {
            index: idx,
            pat: Pat::Ident(first_binding),
        },
    };

    let mut accesses = vec![first_access];
    let mut removed_temps = vec![temp_key, ref_key.clone()];
    let collected = collect_assignment_accesses_on(
        stmts,
        index + 2,
        &ref_binding.id,
        unresolved_mark,
        &mut removed_temps,
    );
    let consumed = 2 + collected.consumed;
    accesses.extend(collected.accesses);

    // The shared `ref` binding must be fully consumed by this group.
    if ident_used_in_stmts(&stmts[index + consumed..], &ref_key) {
        return None;
    }

    let nested_pat = build_pat_from_accesses(accesses)?;
    let pat = Pat::Assign(AssignPat {
        span: DUMMY_SP,
        left: Box::new(nested_pat),
        right: default,
    });
    let access = match source_access {
        SourceAccess::ArrayIndex(idx) => Access::Array { index: idx, pat },
        SourceAccess::ObjectProp(key) => Access::Object { key, pat },
    };

    Some(AssignmentAccess {
        source,
        init: source_init,
        access,
        consumed,
        removed_temps,
    })
}

fn try_extract_assignment_default_access(
    stmts: &[Stmt],
    index: usize,
    expected_source: Option<&Ident>,
    unresolved_mark: Mark,
) -> Option<AssignmentAccess> {
    let (temp, temp_init) = extract_binding_assignment(stmts.get(index)?)?;
    let (source, source_init, source_access) =
        extract_assignment_source_access(temp_init, expected_source)?;

    let (binding, binding_init) = extract_binding_assignment(stmts.get(index + 1)?)?;
    let default = extract_default_value(binding_init, &temp.id, unresolved_mark)?;
    let temp_key = binding_key(&temp.id);
    if expr_uses_ident(&default, &temp_key) {
        return None;
    }

    let pat = Pat::Assign(AssignPat {
        span: DUMMY_SP,
        left: Box::new(Pat::Ident(binding)),
        right: default,
    });

    let access = match source_access {
        SourceAccess::ArrayIndex(index) => Access::Array { index, pat },
        SourceAccess::ObjectProp(key) => Access::Object { key, pat },
    };

    Some(AssignmentAccess {
        source,
        init: source_init,
        access,
        consumed: 2,
        removed_temps: vec![temp_key],
    })
}

fn try_nest_assignment_default_binding(
    access: &Access,
    stmts: &[Stmt],
    nested_start: usize,
    unresolved_mark: Mark,
    removed_temps: &mut Vec<BindingKey>,
) -> Option<(Pat, usize)> {
    let default_binding = match access {
        Access::Object { pat, .. } | Access::Array { pat, .. } => {
            let Pat::Assign(assign) = pat else {
                return None;
            };
            let Pat::Ident(binding) = assign.left.as_ref() else {
                return None;
            };
            &binding.id
        }
        Access::ArrayRest { .. } => return None,
    };

    let collected = collect_assignment_accesses_on(
        stmts,
        nested_start,
        default_binding,
        unresolved_mark,
        removed_temps,
    );

    if collected.accesses.is_empty() {
        return None;
    }

    let after_nested = nested_start + collected.consumed;
    let nested_key = binding_key(default_binding);
    if ident_used_in_stmts(&stmts[after_nested..], &nested_key) {
        return None;
    }

    removed_temps.push(nested_key);
    let nested_pat = build_pat_from_accesses(collected.accesses)?;
    Some((nested_pat, collected.consumed))
}

fn collect_assignment_accesses_on(
    stmts: &[Stmt],
    start: usize,
    ref_ident: &Ident,
    unresolved_mark: Mark,
    removed_temps: &mut Vec<BindingKey>,
) -> CollectedAccesses {
    let mut accesses = Vec::new();
    let mut i = start;

    while i < stmts.len() {
        if let Some(extracted) =
            try_extract_assignment_access(stmts, i, Some(ref_ident), unresolved_mark)
        {
            accesses.push(extracted.access);
            removed_temps.extend(extracted.removed_temps);
            i += extracted.consumed;
        } else if let Some((nested, consumed)) =
            try_expand_sliced_to_array_accesses(stmts, i, ref_ident, unresolved_mark, removed_temps)
        {
            accesses.extend(nested);
            i += consumed;
        } else {
            break;
        }
    }

    CollectedAccesses {
        accesses,
        consumed: i - start,
    }
}

/// When we encounter `ref = _slicedToArray(expected_source, N)` followed by
/// `binding = ref[i]` assignments, expand the `_slicedToArray` transparently:
/// collect the index accesses on `ref` and return them as direct array accesses
/// on `expected_source`. This lets `UnDestructuring` fold
/// `tags: temp = []; ref = _slicedToArray(temp, 3); a = ref[0]; c = ref[2]`
/// into `tags: [a, , c] = []`.
fn try_expand_sliced_to_array_accesses(
    stmts: &[Stmt],
    index: usize,
    expected_source: &Ident,
    unresolved_mark: Mark,
    removed_temps: &mut Vec<BindingKey>,
) -> Option<(Vec<Access>, usize)> {
    let (ref_binding, ref_init) = extract_binding_assignment(stmts.get(index)?)?;
    let Expr::Call(call) = strip_parens(ref_init) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !is_sliced_to_array_callee_name(callee.as_ref()) || call.args.len() != 2 {
        return None;
    }
    let source_arg = strip_parens(call.args[0].expr.as_ref());
    let Expr::Ident(source_ident) = source_arg else {
        return None;
    };
    if source_ident.sym != expected_source.sym || source_ident.ctxt != expected_source.ctxt {
        return None;
    }

    let ref_key = binding_key(&ref_binding.id);
    removed_temps.push(ref_key);
    let collected = collect_assignment_accesses_on(
        stmts,
        index + 1,
        &ref_binding.id,
        unresolved_mark,
        removed_temps,
    );
    if collected.accesses.is_empty() {
        return None;
    }
    let after = index + 1 + collected.consumed;
    if ident_used_in_stmts(&stmts[after..], &binding_key(&ref_binding.id)) {
        return None;
    }
    Some((collected.accesses, 1 + collected.consumed))
}

fn try_reconstruct_ref_group(
    stmts: &[Stmt],
    start: usize,
    unresolved_mark: Mark,
    consumed_helpers: &mut Vec<BindingKey>,
) -> Option<ReconstructedGroup> {
    let ref_decl = extract_ref_decl(stmts.get(start)?)?;
    let ref_key = binding_key(&ref_decl.ident.id);

    let mut removed_temps = Vec::new();
    let collected = collect_accesses_on(
        stmts,
        start + 1,
        &ref_decl.ident.id,
        unresolved_mark,
        &mut removed_temps,
        consumed_helpers,
    );

    if collected.accesses.is_empty() {
        return None;
    }
    if !collected.accesses.iter().any(is_rest_or_default_access) {
        return None;
    }

    let i = start + 1 + collected.consumed;

    let mut removed_bindings = vec![ref_key.clone()];
    removed_bindings.extend(removed_temps.iter().cloned());
    if collected
        .accesses
        .iter()
        .any(|access| default_uses_any_removed_binding(access, &removed_bindings))
    {
        return None;
    }

    if ident_used_in_stmts(&stmts[i..], &ref_key) {
        return None;
    }
    for temp in &removed_temps {
        if ident_used_in_stmts(&stmts[i..], temp) {
            return None;
        }
    }

    let stmt = build_destructuring_stmt(&ref_decl, collected.accesses)?;
    Some(ReconstructedGroup {
        stmt,
        consumed: i - start,
        remove_prior_bindings: Vec::new(),
    })
}

struct CollectedAccesses {
    accesses: Vec<Access>,
    consumed: usize,
}

fn collect_accesses_on(
    stmts: &[Stmt],
    start: usize,
    ref_ident: &Ident,
    unresolved_mark: Mark,
    removed_temps: &mut Vec<BindingKey>,
    consumed_helpers: &mut Vec<BindingKey>,
) -> CollectedAccesses {
    let mut accesses = Vec::new();
    let mut i = start;

    while i < stmts.len() {
        if let Some((access, consumed)) = try_extract_access(
            stmts,
            i,
            ref_ident,
            unresolved_mark,
            removed_temps,
            consumed_helpers,
        ) {
            accesses.push(access);
            i += consumed;
        } else {
            break;
        }
    }

    CollectedAccesses {
        accesses,
        consumed: i - start,
    }
}

fn is_rest_or_default_access(access: &Access) -> bool {
    match access {
        Access::ArrayRest { .. } => true,
        Access::Array { pat, .. } | Access::Object { pat, .. } => matches!(pat, Pat::Assign(_)),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DeclGroup {
    span: Span,
    ctxt: SyntaxContext,
    kind: VarDeclKind,
    declare: bool,
}

fn try_reconstruct_direct_array_group(
    stmts: &[Stmt],
    start: usize,
    unresolved_mark: Mark,
) -> Option<ReconstructedGroup> {
    let (source, group, first_access, consumed, first_temp) =
        try_extract_direct_array_access(stmts, start, None, None, unresolved_mark)?;

    let mut accesses = vec![first_access];
    let mut removed_temps = Vec::new();
    if let Some(temp) = first_temp {
        removed_temps.push(temp);
    }

    let mut i = start + consumed;
    while i < stmts.len() {
        let next_default = try_extract_direct_default_access(
            stmts,
            i,
            Some(&source),
            Some(group),
            unresolved_mark,
        )
        .map(|(source, group, access, temp)| (source, group, access, 2, Some(temp)));

        let next_access = next_default.or_else(|| {
            try_extract_direct_array_access(stmts, i, Some(&source), Some(group), unresolved_mark)
        });

        if let Some((_, _, access, consumed, temp)) = next_access {
            accesses.push(access);
            if let Some(temp) = temp {
                removed_temps.push(temp);
            }
            i += consumed;
        } else {
            break;
        }
    }

    if !accesses
        .iter()
        .any(|access| matches!(access, Access::ArrayRest { .. }))
        || !accesses
            .iter()
            .any(|access| matches!(access, Access::Array { .. }))
    {
        return None;
    }

    if accesses
        .iter()
        .any(|access| default_uses_any_removed_binding(access, &removed_temps))
    {
        return None;
    }

    for temp in &removed_temps {
        if ident_used_in_stmts(&stmts[i..], temp) {
            return None;
        }
    }

    let stmt =
        build_direct_array_destructuring_stmt(group, accesses, Box::new(Expr::Ident(source)))?;
    Some(ReconstructedGroup {
        stmt,
        consumed: i - start,
        remove_prior_bindings: Vec::new(),
    })
}

fn try_extract_direct_array_access(
    stmts: &[Stmt],
    index: usize,
    expected_source: Option<&Ident>,
    expected_group: Option<DeclGroup>,
    unresolved_mark: Mark,
) -> Option<(Ident, DeclGroup, Access, usize, Option<BindingKey>)> {
    if let Some((source, group, access, temp)) = try_extract_direct_default_access(
        stmts,
        index,
        expected_source,
        expected_group,
        unresolved_mark,
    ) {
        return Some((source, group, access, 2, Some(temp)));
    }

    let (group, binding, init) = extract_grouped_binding_decl(stmts.get(index)?)?;
    if let Some(expected_group) = expected_group {
        if group != expected_group {
            return None;
        }
    }

    if let Some((source, index)) = extract_direct_array_index(init, expected_source) {
        return Some((
            source,
            group,
            Access::Array {
                index,
                pat: Pat::Ident(binding),
            },
            1,
            None,
        ));
    }

    if let Some((source, start, binding)) =
        extract_direct_slice_rest(init, expected_source, binding)
    {
        return Some((source, group, Access::ArrayRest { start, binding }, 1, None));
    }

    None
}

fn try_extract_direct_default_access(
    stmts: &[Stmt],
    index: usize,
    expected_source: Option<&Ident>,
    expected_group: Option<DeclGroup>,
    unresolved_mark: Mark,
) -> Option<(Ident, DeclGroup, Access, BindingKey)> {
    let (group, temp, temp_init) = extract_grouped_binding_decl(stmts.get(index)?)?;
    if let Some(expected_group) = expected_group {
        if group != expected_group {
            return None;
        }
    }
    let (source, array_index) = extract_direct_array_index(temp_init, expected_source)?;

    let (next_group, binding, binding_init) = extract_grouped_binding_decl(stmts.get(index + 1)?)?;
    if expected_group.is_some() && next_group != group {
        return None;
    }
    let default = extract_default_value(binding_init, &temp.id, unresolved_mark)?;
    let temp_key = binding_key(&temp.id);
    if expr_uses_ident(&default, &temp_key) {
        return None;
    }

    let pat = Pat::Assign(AssignPat {
        span: DUMMY_SP,
        left: Box::new(Pat::Ident(binding)),
        right: default,
    });

    Some((
        source,
        group,
        Access::Array {
            index: array_index,
            pat,
        },
        temp_key,
    ))
}

fn extract_direct_array_index(
    expr: &Expr,
    expected_source: Option<&Ident>,
) -> Option<(Ident, usize)> {
    let Expr::Member(member) = expr else {
        return None;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return None;
    };
    if let Some(expected) = expected_source {
        if obj.sym != expected.sym || obj.ctxt != expected.ctxt {
            return None;
        }
    }
    let MemberProp::Computed(computed) = &member.prop else {
        return None;
    };
    let Expr::Lit(Lit::Num(num)) = computed.expr.as_ref() else {
        return None;
    };
    Some((obj.clone(), numeric_index(num)?))
}

fn extract_direct_slice_rest(
    expr: &Expr,
    expected_source: Option<&Ident>,
    binding: BindingIdent,
) -> Option<(Ident, usize, BindingIdent)> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if call.args.len() != 1 {
        return None;
    }
    let swc_core::ecma::ast::Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = callee.as_ref() else {
        return None;
    };
    let Expr::Ident(source) = obj.as_ref() else {
        return None;
    };
    if let Some(expected) = expected_source {
        if source.sym != expected.sym || source.ctxt != expected.ctxt {
            return None;
        }
    }
    if !matches!(prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "slice") {
        return None;
    }
    let Expr::Lit(Lit::Num(num)) = call.args[0].expr.as_ref() else {
        return None;
    };
    Some((source.clone(), numeric_index(num)?, binding))
}

fn default_uses_any_removed_binding(access: &Access, removed_bindings: &[BindingKey]) -> bool {
    match access {
        Access::Array { pat, .. } | Access::Object { pat, .. } => {
            default_pat_uses_any_removed_binding(pat, removed_bindings)
        }
        Access::ArrayRest { .. } => false,
    }
}

fn default_pat_uses_any_removed_binding(pat: &Pat, removed_bindings: &[BindingKey]) -> bool {
    let Pat::Assign(assign) = pat else {
        return false;
    };
    removed_bindings
        .iter()
        .any(|binding| expr_uses_ident(&assign.right, binding))
}

fn extract_ref_decl(stmt: &Stmt) -> Option<RefDecl> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(ident) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;

    Some(RefDecl {
        span: var.span,
        ctxt: var.ctxt,
        kind: var.kind,
        declare: var.declare,
        ident: ident.clone(),
        init: unwrap_spread_array_source(init),
    })
}

fn unwrap_spread_array_source(expr: &Expr) -> Box<Expr> {
    if let Expr::Array(array) = expr {
        if array.elems.len() == 1 {
            if let Some(ExprOrSpread {
                spread: Some(_),
                expr,
            }) = &array.elems[0]
            {
                return expr.clone();
            }
        }
    }
    Box::new(expr.clone())
}

fn try_extract_access(
    stmts: &[Stmt],
    index: usize,
    ref_ident: &Ident,
    unresolved_mark: Mark,
    removed_temps: &mut Vec<BindingKey>,
    consumed_helpers: &mut Vec<BindingKey>,
) -> Option<(Access, usize)> {
    if let Some((mut access, temp)) =
        try_extract_default_access(stmts, index, ref_ident, unresolved_mark)
    {
        removed_temps.push(temp);
        let mut consumed = 2;

        if let Some((nested_pat, extra)) = try_nest_default_binding(
            &access,
            stmts,
            index + 2,
            unresolved_mark,
            removed_temps,
            consumed_helpers,
        ) {
            replace_access_left(&mut access, nested_pat);
            consumed += extra;
        }

        return Some((access, consumed));
    }

    let (binding, init) = extract_binding_decl(stmts.get(index)?)?;
    if let Some(source) = extract_source_access(init, ref_ident) {
        let access = match source {
            SourceAccess::ArrayIndex(index) => Access::Array {
                index,
                pat: Pat::Ident(binding),
            },
            SourceAccess::ObjectProp(key) => Access::Object {
                key,
                pat: Pat::Ident(binding),
            },
        };
        return Some((access, 1));
    }

    if let Some((start, binding, helper_key)) = extract_slice_rest(init, ref_ident, binding) {
        if let Some(key) = helper_key {
            consumed_helpers.push(key);
        }
        return Some((Access::ArrayRest { start, binding }, 1));
    }

    None
}

fn try_nest_default_binding(
    access: &Access,
    stmts: &[Stmt],
    nested_start: usize,
    unresolved_mark: Mark,
    removed_temps: &mut Vec<BindingKey>,
    consumed_helpers: &mut Vec<BindingKey>,
) -> Option<(Pat, usize)> {
    let default_binding = match access {
        Access::Object { pat, .. } | Access::Array { pat, .. } => {
            let Pat::Assign(assign) = pat else {
                return None;
            };
            let Pat::Ident(binding) = assign.left.as_ref() else {
                return None;
            };
            &binding.id
        }
        Access::ArrayRest { .. } => return None,
    };

    let collected = collect_accesses_on(
        stmts,
        nested_start,
        default_binding,
        unresolved_mark,
        removed_temps,
        consumed_helpers,
    );

    if collected.accesses.is_empty() || !collected.accesses.iter().any(is_rest_or_default_access) {
        return None;
    }

    let after_nested = nested_start + collected.consumed;
    let nested_key = binding_key(default_binding);
    if ident_used_in_stmts(&stmts[after_nested..], &nested_key) {
        return None;
    }

    removed_temps.push(nested_key);
    let nested_pat = build_pat_from_accesses(collected.accesses)?;
    Some((nested_pat, collected.consumed))
}

fn replace_access_left(access: &mut Access, nested_pat: Pat) {
    let pat = match access {
        Access::Object { pat, .. } | Access::Array { pat, .. } => pat,
        Access::ArrayRest { .. } => return,
    };
    let Pat::Assign(assign) = pat else { return };
    *assign.left = nested_pat;
}

fn try_extract_default_access(
    stmts: &[Stmt],
    index: usize,
    ref_ident: &Ident,
    unresolved_mark: Mark,
) -> Option<(Access, BindingKey)> {
    let (temp, temp_init) = extract_binding_decl(stmts.get(index)?)?;
    let source = extract_source_access(temp_init, ref_ident)?;

    let (binding, binding_init) = extract_binding_decl(stmts.get(index + 1)?)?;
    let default = extract_default_value(binding_init, &temp.id, unresolved_mark)?;
    let temp_key = binding_key(&temp.id);
    if expr_uses_ident(&default, &temp_key) {
        return None;
    }

    let pat = Pat::Assign(AssignPat {
        span: DUMMY_SP,
        left: Box::new(Pat::Ident(binding)),
        right: default,
    });

    let access = match source {
        SourceAccess::ArrayIndex(index) => Access::Array { index, pat },
        SourceAccess::ObjectProp(key) => Access::Object { key, pat },
    };

    Some((access, temp_key))
}

fn extract_binding_decl(stmt: &Stmt) -> Option<(BindingIdent, &Expr)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    Some((binding.clone(), decl.init.as_deref()?))
}

fn extract_binding_assignment(stmt: &Stmt) -> Option<(BindingIdent, &Expr)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(expr.as_ref()) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assign.left else {
        return None;
    };
    Some((binding.clone(), strip_parens(assign.right.as_ref())))
}

fn extract_grouped_binding_decl(stmt: &Stmt) -> Option<(DeclGroup, BindingIdent, &Expr)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    Some((
        DeclGroup {
            span: var.span,
            ctxt: var.ctxt,
            kind: var.kind,
            declare: var.declare,
        },
        binding.clone(),
        decl.init.as_deref()?,
    ))
}

fn extract_assignment_source_access(
    expr: &Expr,
    expected_source: Option<&Ident>,
) -> Option<(Ident, Box<Expr>, SourceAccess)> {
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    let (source, source_init) =
        extract_assignment_source_expr(member.obj.as_ref(), expected_source)?;

    let access = match &member.prop {
        MemberProp::Ident(prop) => SourceAccess::ObjectProp(PropKey::Ident(prop.sym.clone())),
        MemberProp::Computed(computed) => match strip_parens(computed.expr.as_ref()) {
            Expr::Lit(Lit::Num(num)) => SourceAccess::ArrayIndex(numeric_index(num)?),
            Expr::Lit(Lit::Str(s)) => {
                SourceAccess::ObjectProp(PropKey::Str(s.value.as_str().map(Atom::from)?))
            }
            _ => return None,
        },
        _ => return None,
    };

    Some((source, source_init, access))
}

fn extract_default_member_access(
    expr: &Expr,
    temp: &Ident,
    unresolved_mark: Mark,
) -> Option<(Box<Expr>, SourceAccess)> {
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    let default = extract_default_value(strip_parens(member.obj.as_ref()), temp, unresolved_mark)?;
    let access = source_access_from_member_prop(&member.prop)?;
    Some((default, access))
}

fn source_access_from_member_prop(prop: &MemberProp) -> Option<SourceAccess> {
    match prop {
        MemberProp::Ident(prop) => Some(SourceAccess::ObjectProp(PropKey::Ident(prop.sym.clone()))),
        MemberProp::Computed(computed) => match strip_parens(computed.expr.as_ref()) {
            Expr::Lit(Lit::Num(num)) => numeric_index(num).map(SourceAccess::ArrayIndex),
            Expr::Lit(Lit::Str(s)) => s
                .value
                .as_str()
                .map(|value| SourceAccess::ObjectProp(PropKey::Str(value.into()))),
            _ => None,
        },
        _ => None,
    }
}

fn is_sliced_to_array_callee_name(expr: &Expr) -> bool {
    let Expr::Ident(callee) = strip_parens(expr) else {
        return false;
    };
    matches!(
        callee.sym.as_ref(),
        "_sliced_to_array" | "_slicedToArray" | "_slicedToArrayLoose" | "_sliced_to_array_loose"
    )
}

fn extract_assignment_source_expr(
    expr: &Expr,
    expected_source: Option<&Ident>,
) -> Option<(Ident, Box<Expr>)> {
    match strip_parens(expr) {
        Expr::Ident(source) => {
            if let Some(expected) = expected_source {
                if source.sym != expected.sym || source.ctxt != expected.ctxt {
                    return None;
                }
            }
            Some((source.clone(), Box::new(Expr::Ident(source.clone()))))
        }
        Expr::Assign(assign) if expected_source.is_none() && assign.op == AssignOp::Assign => {
            let AssignTarget::Simple(SimpleAssignTarget::Ident(source)) = &assign.left else {
                return None;
            };
            Some((source.id.clone(), Box::new(Expr::Assign(assign.clone()))))
        }
        _ => None,
    }
}

fn extract_source_access(expr: &Expr, ref_ident: &Ident) -> Option<SourceAccess> {
    let Expr::Member(member) = expr else {
        return None;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return None;
    };
    if obj.sym != ref_ident.sym || obj.ctxt != ref_ident.ctxt {
        return None;
    }

    match &member.prop {
        MemberProp::Ident(prop) => Some(SourceAccess::ObjectProp(PropKey::Ident(prop.sym.clone()))),
        MemberProp::Computed(computed) => match computed.expr.as_ref() {
            Expr::Lit(Lit::Num(num)) => numeric_index(num).map(SourceAccess::ArrayIndex),
            Expr::Lit(Lit::Str(s)) => s
                .value
                .as_str()
                .map(|value| SourceAccess::ObjectProp(PropKey::Str(value.into()))),
            _ => None,
        },
        _ => None,
    }
}

fn extract_slice_rest(
    expr: &Expr,
    ref_ident: &Ident,
    binding: BindingIdent,
) -> Option<(usize, BindingIdent, Option<BindingKey>)> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if call.args.len() != 1 {
        return None;
    }
    let swc_core::ecma::ast::Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = callee.as_ref() else {
        return None;
    };
    let helper_key = match_ref_or_array_like_to_array(obj, ref_ident)?;
    if !matches!(prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "slice") {
        return None;
    }
    let Expr::Lit(Lit::Num(num)) = call.args[0].expr.as_ref() else {
        return None;
    };
    Some((numeric_index(num)?, binding, helper_key))
}

/// Checks whether `expr` is `ref_ident` or `_arrayLikeToArray(ref_ident)`.
/// Returns `Some(helper_binding_key)` when the `_arrayLikeToArray` wrapper was
/// matched, `Some(None)` for a direct ref match, or `None` on mismatch.
fn match_ref_or_array_like_to_array(expr: &Expr, ref_ident: &Ident) -> Option<Option<BindingKey>> {
    match expr {
        Expr::Ident(obj) if obj.sym == ref_ident.sym && obj.ctxt == ref_ident.ctxt => Some(None),
        Expr::Call(call) => {
            if call.args.is_empty() || call.args[0].spread.is_some() {
                return None;
            }
            let swc_core::ecma::ast::Callee::Expr(callee) = &call.callee else {
                return None;
            };
            let Expr::Ident(helper) = callee.as_ref() else {
                return None;
            };
            if !matches!(
                helper.sym.as_ref(),
                "_arrayLikeToArray" | "_array_like_to_array"
            ) {
                return None;
            }
            match_ref_or_array_like_to_array(call.args[0].expr.as_ref(), ref_ident)?;
            Some(Some(binding_key(helper)))
        }
        _ => None,
    }
}

fn numeric_index(num: &Number) -> Option<usize> {
    if num.value < 0.0 || num.value.fract() != 0.0 || num.value > 64.0 {
        return None;
    }
    Some(num.value as usize)
}

/// Extract the default value from the first argument of `_slicedToArray()`.
fn extract_sliced_default_arg(
    expr: &Expr,
    temp: &Ident,
    unresolved_mark: Mark,
) -> Option<(Box<Expr>, Option<Ident>)> {
    if let Some(default) = extract_default_value(expr, temp, unresolved_mark) {
        return Some((default, None));
    }
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(fused_binding)) = &assign.left else {
        return None;
    };
    let default =
        extract_default_value(strip_parens(assign.right.as_ref()), temp, unresolved_mark)?;
    Some((default, Some(fused_binding.id.clone())))
}

fn extract_default_value(expr: &Expr, temp: &Ident, unresolved_mark: Mark) -> Option<Box<Expr>> {
    extract_ternary_default(expr, temp, unresolved_mark)
        .or_else(|| extract_boolean_default(expr, temp, unresolved_mark))
}

fn extract_ternary_default(expr: &Expr, temp: &Ident, unresolved_mark: Mark) -> Option<Box<Expr>> {
    let Expr::Cond(cond) = expr else {
        return None;
    };
    if !is_undefined_test(&cond.test, temp, unresolved_mark) || !is_ident_expr(&cond.alt, temp) {
        return None;
    }
    Some(cond.cons.clone())
}

fn extract_boolean_default(expr: &Expr, temp: &Ident, unresolved_mark: Mark) -> Option<Box<Expr>> {
    let Expr::Bin(bin) = expr else {
        return None;
    };

    match bin.op {
        BinaryOp::LogicalAnd
            if is_defined_test(&bin.left, temp, unresolved_mark)
                && is_ident_expr(&bin.right, temp) =>
        {
            Some(bool_expr(false))
        }
        BinaryOp::LogicalOr
            if is_undefined_test(&bin.left, temp, unresolved_mark)
                && is_ident_expr(&bin.right, temp) =>
        {
            Some(bool_expr(true))
        }
        _ => None,
    }
}

fn bool_expr(value: bool) -> Box<Expr> {
    Box::new(Expr::Lit(Lit::Bool(Bool {
        span: DUMMY_SP,
        value,
    })))
}

fn is_undefined_test(expr: &Expr, temp: &Ident, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = expr else {
        return false;
    };
    bin.op == BinaryOp::EqEqEq
        && ((is_ident_expr(&bin.left, temp)
            && is_unresolved_undefined(&bin.right, unresolved_mark))
            || (is_unresolved_undefined(&bin.left, unresolved_mark)
                && is_ident_expr(&bin.right, temp)))
}

fn is_defined_test(expr: &Expr, temp: &Ident, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = expr else {
        return false;
    };
    bin.op == BinaryOp::NotEqEq
        && ((is_ident_expr(&bin.left, temp)
            && is_unresolved_undefined(&bin.right, unresolved_mark))
            || (is_unresolved_undefined(&bin.left, unresolved_mark)
                && is_ident_expr(&bin.right, temp)))
}

fn is_ident_expr(expr: &Expr, ident: &Ident) -> bool {
    matches!(expr, Expr::Ident(id) if id.sym == ident.sym && id.ctxt == ident.ctxt)
}

fn build_destructuring_stmt(ref_decl: &RefDecl, accesses: Vec<Access>) -> Option<Stmt> {
    let pat = build_pat_from_accesses(accesses)?;
    Some(build_var_stmt(ref_decl, pat))
}

fn build_pat_from_accesses(accesses: Vec<Access>) -> Option<Pat> {
    if accesses
        .iter()
        .all(|access| matches!(access, Access::Array { .. } | Access::ArrayRest { .. }))
    {
        build_array_pat(accesses)
    } else if accesses
        .iter()
        .all(|access| matches!(access, Access::Object { .. }))
    {
        build_object_pat(accesses)
    } else {
        None
    }
}

fn build_direct_array_destructuring_stmt(
    group: DeclGroup,
    accesses: Vec<Access>,
    init: Box<Expr>,
) -> Option<Stmt> {
    let pat = build_array_pat(accesses)?;
    Some(build_var_stmt_from_parts(
        group.span,
        group.ctxt,
        group.kind,
        group.declare,
        pat,
        init,
    ))
}

fn build_assignment_destructuring_stmt(accesses: Vec<Access>, init: Box<Expr>) -> Option<Stmt> {
    let pat = build_pat_from_accesses(accesses)?;
    let left = match pat {
        Pat::Array(array) => AssignTarget::Pat(AssignTargetPat::Array(array)),
        Pat::Object(object) => AssignTarget::Pat(AssignTargetPat::Object(object)),
        _ => return None,
    };

    Some(Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left,
            right: init,
        })),
    }))
}

fn build_array_pat(accesses: Vec<Access>) -> Option<Pat> {
    if accesses
        .iter()
        .filter(|access| matches!(access, Access::ArrayRest { .. }))
        .count()
        > 1
    {
        return None;
    }

    let rest = accesses.iter().find_map(|access| {
        if let Access::ArrayRest { start, binding } = access {
            Some((*start, binding.clone()))
        } else {
            None
        }
    });

    let max_index = accesses
        .iter()
        .filter_map(|access| match access {
            Access::Array { index, .. } => Some(*index),
            _ => None,
        })
        .max()
        .unwrap_or(0);

    if let Some((rest_start, _)) = &rest {
        if accesses
            .iter()
            .any(|access| matches!(access, Access::Array { index, .. } if index >= rest_start))
        {
            return None;
        }
    }

    let elem_len = rest
        .as_ref()
        .map(|(start, _)| start + 1)
        .unwrap_or(max_index + 1);
    let mut elems: Vec<Option<Pat>> = vec![None; elem_len];

    for access in accesses {
        match access {
            Access::Array { index, pat } => {
                if elems[index].is_some() {
                    return None;
                }
                elems[index] = Some(pat);
            }
            Access::ArrayRest { start, binding } => {
                if elems[start].is_some() {
                    return None;
                }
                elems[start] = Some(Pat::Rest(RestPat {
                    span: DUMMY_SP,
                    dot3_token: DUMMY_SP,
                    arg: Box::new(Pat::Ident(binding)),
                    type_ann: None,
                }));
            }
            Access::Object { .. } => return None,
        }
    }

    Some(Pat::Array(ArrayPat {
        span: DUMMY_SP,
        elems,
        optional: false,
        type_ann: None,
    }))
}

fn build_object_pat(accesses: Vec<Access>) -> Option<Pat> {
    let mut props = Vec::with_capacity(accesses.len());

    for access in accesses {
        let Access::Object { key, pat } = access else {
            return None;
        };
        props.push(build_object_prop(key, pat));
    }

    Some(Pat::Object(ObjectPat {
        span: DUMMY_SP,
        props,
        optional: false,
        type_ann: None,
    }))
}

fn build_object_prop(key: PropKey, pat: Pat) -> ObjectPatProp {
    let prop_sym = match &key {
        PropKey::Ident(sym) | PropKey::Str(sym) => sym.clone(),
    };

    if let Pat::Ident(binding) = &pat {
        if binding.id.sym == prop_sym && matches!(key, PropKey::Ident(_)) {
            return ObjectPatProp::Assign(AssignPatProp {
                span: DUMMY_SP,
                key: binding.clone(),
                value: None,
            });
        }
    }

    if let Pat::Assign(assign) = &pat {
        if let Pat::Ident(binding) = assign.left.as_ref() {
            if binding.id.sym == prop_sym && matches!(key, PropKey::Ident(_)) {
                return ObjectPatProp::Assign(AssignPatProp {
                    span: DUMMY_SP,
                    key: binding.clone(),
                    value: Some(assign.right.clone()),
                });
            }
        }
    }

    ObjectPatProp::KeyValue(KeyValuePatProp {
        key: prop_name(key),
        value: Box::new(pat),
    })
}

fn prop_name(key: PropKey) -> PropName {
    match key {
        PropKey::Ident(sym) => PropName::Ident(IdentName::new(sym, DUMMY_SP)),
        PropKey::Str(sym) => PropName::Str(swc_core::ecma::ast::Str {
            span: DUMMY_SP,
            value: sym.as_str().into(),
            raw: None,
        }),
    }
}

fn build_var_stmt(ref_decl: &RefDecl, pat: Pat) -> Stmt {
    build_var_stmt_from_parts(
        ref_decl.span,
        ref_decl.ctxt,
        ref_decl.kind,
        ref_decl.declare,
        pat,
        ref_decl.init.clone(),
    )
}

fn build_var_stmt_from_parts(
    span: swc_core::common::Span,
    ctxt: swc_core::common::SyntaxContext,
    kind: VarDeclKind,
    declare: bool,
    pat: Pat,
    init: Box<Expr>,
) -> Stmt {
    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span,
        ctxt,
        kind,
        declare,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: pat,
            init: Some(init),
            definite: false,
        }],
    })))
}

fn nest_param_destructuring(params: &mut [Param], body: &mut BlockStmt, unresolved_mark: Mark) {
    for param in params.iter_mut() {
        nest_pat_destructuring(&mut param.pat, &mut body.stmts, unresolved_mark);
    }
}

fn nest_pat_destructuring(pat: &mut Pat, stmts: &mut Vec<Stmt>, unresolved_mark: Mark) {
    let inner_pat = match pat {
        Pat::Assign(assign) => &mut *assign.left,
        other => other,
    };
    let props = match inner_pat {
        Pat::Object(obj) => &mut obj.props,
        _ => return,
    };

    for prop in props.iter_mut() {
        let value_pat = match prop {
            ObjectPatProp::KeyValue(kv) => &mut *kv.value,
            _ => continue,
        };
        let Pat::Assign(assign) = value_pat else {
            continue;
        };
        let Pat::Ident(binding) = assign.left.as_ref() else {
            continue;
        };

        let mut removed_temps = Vec::new();
        let mut consumed_helpers = Vec::new();
        let collected = collect_accesses_on(
            stmts,
            0,
            &binding.id,
            unresolved_mark,
            &mut removed_temps,
            &mut consumed_helpers,
        );

        if collected.accesses.is_empty()
            || !collected.accesses.iter().any(is_rest_or_default_access)
        {
            continue;
        }

        let nested_key = binding_key(&binding.id);
        if ident_used_in_stmts(&stmts[collected.consumed..], &nested_key) {
            continue;
        }
        for temp in &removed_temps {
            if ident_used_in_stmts(&stmts[collected.consumed..], temp) {
                continue;
            }
        }

        let Some(nested_pat) = build_pat_from_accesses(collected.accesses) else {
            continue;
        };

        *assign.left = nested_pat;
        stmts.drain(0..collected.consumed);
        return;
    }
}

/// Remove function/var declarations for helper bindings that are no longer
/// referenced after destructuring reconstruction consumed their call sites.
fn remove_unreferenced_helpers(stmts: &mut Vec<Stmt>, helpers: &[BindingKey]) {
    use std::collections::HashSet;
    let helper_set: HashSet<&BindingKey> = helpers.iter().collect();

    // Collect which helpers are still referenced outside their own declaration.
    let mut referenced: HashSet<&BindingKey> = HashSet::new();
    for stmt in stmts.iter() {
        let declaring = stmt_declares_binding(stmt);
        for key in &helper_set {
            if declaring.as_ref() == Some(*key) {
                continue;
            }
            if stmt_uses_binding(stmt, key) {
                referenced.insert(*key);
            }
        }
    }

    let dead: HashSet<&BindingKey> = helper_set.difference(&referenced).copied().collect();
    if dead.is_empty() {
        return;
    }
    stmts.retain(|stmt| {
        if let Some(key) = stmt_declares_binding(stmt) {
            !dead.contains(&key)
        } else {
            true
        }
    });
}

fn stmt_declares_binding(stmt: &Stmt) -> Option<BindingKey> {
    match stmt {
        Stmt::Decl(Decl::Fn(fn_decl)) => Some(binding_key(&fn_decl.ident)),
        Stmt::Decl(Decl::Var(var_decl)) if var_decl.decls.len() == 1 => {
            let Pat::Ident(ident) = &var_decl.decls[0].name else {
                return None;
            };
            Some(binding_key(&ident.id))
        }
        _ => None,
    }
}

fn stmt_uses_binding(stmt: &Stmt, key: &BindingKey) -> bool {
    let mut finder = IdentUseFinder {
        key: (*key).clone(),
        found: false,
    };
    stmt.visit_with(&mut finder);
    finder.found
}

fn ident_used_in_stmts(stmts: &[Stmt], key: &BindingKey) -> bool {
    let mut finder = IdentUseFinder {
        key: key.clone(),
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

fn expr_uses_ident(expr: &Expr, key: &BindingKey) -> bool {
    let mut finder = IdentUseFinder {
        key: key.clone(),
        found: false,
    };
    expr.visit_with(&mut finder);
    finder.found
}

struct IdentUseFinder {
    key: BindingKey,
    found: bool,
}

impl Visit for IdentUseFinder {
    fn visit_ident(&mut self, ident: &Ident) {
        if binding_key(ident) == self.key {
            self.found = true;
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            computed.visit_with(self);
        }
    }

    fn visit_prop_name(&mut self, _: &PropName) {}
}
