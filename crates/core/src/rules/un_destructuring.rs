use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, AssignPat, AssignPatProp, BinaryOp, BindingIdent, Bool, Decl, Expr, ExprOrSpread,
    Ident, IdentName, KeyValuePatProp, Lit, MemberExpr, MemberProp, Module, ModuleItem, Number,
    ObjectPat, ObjectPatProp, Pat, PropName, RestPat, Stmt, UnaryOp, VarDecl, VarDeclKind,
    VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

/// Reconstructs destructuring from compiler-lowered ref/temp declarations.
///
/// This rule intentionally targets the shape emitted by transforms like SWC's
/// es2015 destructuring pass, rather than guessing from arbitrary property
/// reads. `SmartInline` remains the later readability heuristic for simpler
/// adjacent accesses.
pub struct UnDestructuring;

type BindingKey = (Atom, SyntaxContext);

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
        module.body = process_module_items(std::mem::take(&mut module.body));
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        *stmts = process_stmts(std::mem::take(stmts));
    }
}

fn process_module_items(items: Vec<ModuleItem>) -> Vec<ModuleItem> {
    let mut result = Vec::with_capacity(items.len());
    let mut stmt_buf = Vec::new();

    for item in items {
        match item {
            ModuleItem::Stmt(stmt) => stmt_buf.push(stmt),
            other => {
                if !stmt_buf.is_empty() {
                    result.extend(
                        process_stmts(std::mem::take(&mut stmt_buf))
                            .into_iter()
                            .map(ModuleItem::Stmt),
                    );
                }
                result.push(other);
            }
        }
    }

    if !stmt_buf.is_empty() {
        result.extend(process_stmts(stmt_buf).into_iter().map(ModuleItem::Stmt));
    }

    result
}

fn process_stmts(stmts: Vec<Stmt>) -> Vec<Stmt> {
    let mut result = Vec::with_capacity(stmts.len());
    let mut i = 0;

    while i < stmts.len() {
        if let Some((stmt, consumed)) = try_reconstruct_group(&stmts, i) {
            result.push(stmt);
            i += consumed;
        } else {
            result.push(stmts[i].clone());
            i += 1;
        }
    }

    result
}

fn try_reconstruct_group(stmts: &[Stmt], start: usize) -> Option<(Stmt, usize)> {
    try_reconstruct_ref_group(stmts, start)
}

fn try_reconstruct_ref_group(stmts: &[Stmt], start: usize) -> Option<(Stmt, usize)> {
    let ref_decl = extract_ref_decl(stmts.get(start)?)?;
    let ref_key = binding_key(&ref_decl.ident);

    let mut accesses = Vec::new();
    let mut removed_temps = Vec::new();
    let mut i = start + 1;

    while i < stmts.len() {
        if let Some((access, consumed, temp)) = try_extract_access(stmts, i, &ref_decl.ident.id) {
            accesses.push(access);
            if let Some(temp) = temp {
                removed_temps.push(temp);
            }
            i += consumed;
        } else {
            break;
        }
    }

    if accesses.is_empty() {
        return None;
    }
    if !accesses.iter().any(is_rest_or_default_access) {
        return None;
    }

    let mut removed_bindings = vec![ref_key.clone()];
    removed_bindings.extend(removed_temps.iter().cloned());
    if accesses
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

    let stmt = build_destructuring_stmt(&ref_decl, accesses)?;
    Some((stmt, i - start))
}

fn is_rest_or_default_access(access: &Access) -> bool {
    match access {
        Access::ArrayRest { .. } => true,
        Access::Array { pat, .. } | Access::Object { pat, .. } => matches!(pat, Pat::Assign(_)),
    }
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
) -> Option<(Access, usize, Option<BindingKey>)> {
    if let Some((access, temp)) = try_extract_default_access(stmts, index, ref_ident) {
        return Some((access, 2, Some(temp)));
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
        return Some((access, 1, None));
    }

    if let Some((start, binding)) = extract_slice_rest(init, ref_ident, binding) {
        return Some((Access::ArrayRest { start, binding }, 1, None));
    }

    None
}

fn try_extract_default_access(
    stmts: &[Stmt],
    index: usize,
    ref_ident: &Ident,
) -> Option<(Access, BindingKey)> {
    let (temp, temp_init) = extract_binding_decl(stmts.get(index)?)?;
    let source = extract_source_access(temp_init, ref_ident)?;

    let (binding, binding_init) = extract_binding_decl(stmts.get(index + 1)?)?;
    let default = extract_default_value(binding_init, &temp.id)?;
    let temp_key = binding_key(&temp);
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
) -> Option<(usize, BindingIdent)> {
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
    let Expr::Ident(obj) = obj.as_ref() else {
        return None;
    };
    if obj.sym != ref_ident.sym || obj.ctxt != ref_ident.ctxt {
        return None;
    }
    if !matches!(prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "slice") {
        return None;
    }
    let Expr::Lit(Lit::Num(num)) = call.args[0].expr.as_ref() else {
        return None;
    };
    Some((numeric_index(num)?, binding))
}

fn numeric_index(num: &Number) -> Option<usize> {
    if num.value < 0.0 || num.value.fract() != 0.0 || num.value > 64.0 {
        return None;
    }
    Some(num.value as usize)
}

fn extract_default_value(expr: &Expr, temp: &Ident) -> Option<Box<Expr>> {
    extract_ternary_default(expr, temp).or_else(|| extract_boolean_default(expr, temp))
}

fn extract_ternary_default(expr: &Expr, temp: &Ident) -> Option<Box<Expr>> {
    let Expr::Cond(cond) = expr else {
        return None;
    };
    if !is_undefined_test(&cond.test, temp) || !is_ident_expr(&cond.alt, temp) {
        return None;
    }
    Some(cond.cons.clone())
}

fn extract_boolean_default(expr: &Expr, temp: &Ident) -> Option<Box<Expr>> {
    let Expr::Bin(bin) = expr else {
        return None;
    };

    match bin.op {
        BinaryOp::LogicalAnd
            if is_defined_test(&bin.left, temp) && is_ident_expr(&bin.right, temp) =>
        {
            Some(bool_expr(false))
        }
        BinaryOp::LogicalOr
            if is_undefined_test(&bin.left, temp) && is_ident_expr(&bin.right, temp) =>
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

fn is_undefined_test(expr: &Expr, temp: &Ident) -> bool {
    let Expr::Bin(bin) = expr else {
        return false;
    };
    bin.op == BinaryOp::EqEqEq
        && ((is_ident_expr(&bin.left, temp) && is_undefined_expr(&bin.right))
            || (is_undefined_expr(&bin.left) && is_ident_expr(&bin.right, temp)))
}

fn is_defined_test(expr: &Expr, temp: &Ident) -> bool {
    let Expr::Bin(bin) = expr else {
        return false;
    };
    bin.op == BinaryOp::NotEqEq
        && ((is_ident_expr(&bin.left, temp) && is_undefined_expr(&bin.right))
            || (is_undefined_expr(&bin.left) && is_ident_expr(&bin.right, temp)))
}

fn is_undefined_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Ident(id) => id.sym.as_ref() == "undefined",
        Expr::Unary(unary) => {
            unary.op == UnaryOp::Void
                && matches!(unary.arg.as_ref(), Expr::Lit(Lit::Num(num)) if num.value == 0.0)
        }
        _ => false,
    }
}

fn is_ident_expr(expr: &Expr, ident: &Ident) -> bool {
    matches!(expr, Expr::Ident(id) if id.sym == ident.sym && id.ctxt == ident.ctxt)
}

fn build_destructuring_stmt(ref_decl: &RefDecl, accesses: Vec<Access>) -> Option<Stmt> {
    if accesses
        .iter()
        .all(|access| matches!(access, Access::Array { .. } | Access::ArrayRest { .. }))
    {
        build_array_destructuring_stmt(ref_decl, accesses)
    } else if accesses
        .iter()
        .all(|access| matches!(access, Access::Object { .. }))
    {
        build_object_destructuring_stmt(ref_decl, accesses)
    } else {
        None
    }
}

fn build_array_destructuring_stmt(ref_decl: &RefDecl, accesses: Vec<Access>) -> Option<Stmt> {
    let pat = build_array_pat(accesses)?;
    Some(build_var_stmt(ref_decl, pat))
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

fn build_object_destructuring_stmt(ref_decl: &RefDecl, accesses: Vec<Access>) -> Option<Stmt> {
    let mut props = Vec::with_capacity(accesses.len());

    for access in accesses {
        let Access::Object { key, pat } = access else {
            return None;
        };
        props.push(build_object_prop(key, pat));
    }

    Some(build_var_stmt(
        ref_decl,
        Pat::Object(ObjectPat {
            span: DUMMY_SP,
            props,
            optional: false,
            type_ann: None,
        }),
    ))
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

fn binding_key(binding: &BindingIdent) -> BindingKey {
    (binding.id.sym.clone(), binding.id.ctxt)
}

fn ident_key(ident: &Ident) -> BindingKey {
    (ident.sym.clone(), ident.ctxt)
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
        if ident_key(ident) == self.key {
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
