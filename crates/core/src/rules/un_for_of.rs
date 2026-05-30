use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BindingIdent, BlockStmt,
    CallExpr, Callee, Decl, Expr, ExprOrSpread, ForHead, ForOfStmt, Ident, Lit, MemberExpr,
    MemberProp, ModuleItem, ObjectPatProp, Pat, SimpleAssignTarget, Stmt, TryStmt, UnaryExpr,
    UnaryOp, UpdateExpr, UpdateOp, VarDecl, VarDeclKind, VarDeclOrExpr, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::RewriteLevel;

/// Convert TypeScript/Babel array-index downlevel `for` loops back to `for...of`:
///
/// ```js
/// for (let i = 0, arr = expr; i < arr.length; i++) {
///     const elem = arr[i];
///     // body...
/// }
/// // →
/// for (const elem of expr) {
///     // body...
/// }
/// ```
pub struct UnForOf {
    level: RewriteLevel,
}

impl UnForOf {
    pub fn new(level: RewriteLevel) -> Self {
        Self { level }
    }
}

impl Default for UnForOf {
    fn default() -> Self {
        Self::new(RewriteLevel::Standard)
    }
}

impl VisitMut for UnForOf {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        if self.level < RewriteLevel::Standard {
            return;
        }

        let old = std::mem::take(items);
        let mut stmt_run = Vec::new();

        for item in old {
            match item {
                ModuleItem::Stmt(stmt) => stmt_run.push(stmt),
                item => {
                    flush_stmt_run(items, &mut stmt_run);
                    items.push(item);
                }
            }
        }
        flush_stmt_run(items, &mut stmt_run);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        if self.level < RewriteLevel::Standard {
            return;
        }

        process_stmt_vec(stmts);
    }

    fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        stmt.visit_mut_children_with(self);

        if let Some(for_of) = try_convert_for_of(stmt) {
            *stmt = Stmt::ForOf(for_of);
        }
    }
}

fn flush_stmt_run(items: &mut Vec<ModuleItem>, stmts: &mut Vec<Stmt>) {
    if stmts.is_empty() {
        return;
    }
    process_stmt_vec(stmts);
    items.extend(std::mem::take(stmts).into_iter().map(ModuleItem::Stmt));
}

fn process_stmt_vec(stmts: &mut Vec<Stmt>) {
    let old = std::mem::take(stmts);
    let mut i = 0;
    while i < old.len() {
        if let Some(rewrite) = try_convert_ts_values_sequence(&old[i..]) {
            stmts.push(Stmt::ForOf(rewrite.for_of));
            i += rewrite.consumed_stmts;
            continue;
        }

        if let Some(rewrite) = try_convert_swc_iterator_sequence(&old[i..]) {
            stmts.push(Stmt::ForOf(rewrite.for_of));
            i += rewrite.consumed_stmts;
            continue;
        }

        if let Some(rewrite) = try_convert_iterator_helper_sequence(&old[i..]) {
            stmts.extend(rewrite.preserved_stmts);
            stmts.push(Stmt::ForOf(rewrite.for_of));
            i += rewrite.consumed_stmts;
            continue;
        }

        if let Some(rewrite) = try_convert_loose_iterator_sequence(&old[i..]) {
            stmts.push(Stmt::ForOf(rewrite.for_of));
            i += rewrite.consumed_stmts;
            continue;
        }

        let stmt = old[i].clone();
        if let Some(for_of) = try_convert_for_of(&stmt) {
            stmts.push(Stmt::ForOf(for_of));
        } else {
            stmts.push(stmt);
        }
        i += 1;
    }
}

struct SequenceRewrite {
    consumed_stmts: usize,
    preserved_stmts: Vec<Stmt>,
    for_of: ForOfStmt,
}

fn try_convert_iterator_helper_sequence(stmts: &[Stmt]) -> Option<SequenceRewrite> {
    if let Some(rewrite) = try_convert_iterator_helper_decl_first_sequence(stmts) {
        return Some(rewrite);
    }

    let item_ident = empty_single_var_ident(stmts.first()?)?;

    let mut helper_index = None;
    let mut preserved_stmts = Vec::new();
    for (idx, stmt) in stmts.iter().enumerate().skip(1) {
        if let Some(decl) = stmt_as_single_var_decl(stmt) {
            if decl.decls[0].init.is_some() && pat_as_ident(&decl.decls[0].name).is_some() {
                helper_index = Some(idx);
                break;
            }
        }

        if empty_single_var_ident(stmt).is_some() {
            preserved_stmts.push(stmt.clone());
            continue;
        }

        return None;
    }

    let helper_index = helper_index?;
    let helper_decl = stmt_as_single_var_decl(&stmts[helper_index])?;
    let helper_ident = pat_as_ident(&helper_decl.decls[0].name)?.id.clone();
    let iterable = extract_single_call_arg(helper_decl.decls[0].init.as_ref()?)?;
    let try_stmt = stmt_as_try(stmts.get(helper_index + 1)?)?;
    let helper_loop = extract_iterator_helper_loop(try_stmt, &helper_ident, &item_ident)?;

    let consumed_stmts = helper_index + 2;
    if stmts[consumed_stmts..].iter().any(|stmt| {
        stmt_uses_ident_key(stmt, &item_ident) || stmt_uses_ident_key(stmt, &helper_ident)
    }) {
        return None;
    }

    let for_of = build_helper_for_of(helper_loop, iterable, item_ident)?;
    Some(SequenceRewrite {
        consumed_stmts,
        preserved_stmts,
        for_of,
    })
}

fn try_convert_iterator_helper_decl_first_sequence(stmts: &[Stmt]) -> Option<SequenceRewrite> {
    let helper_decl = stmt_as_single_var_decl(stmts.first()?)?;
    let helper_ident = pat_as_ident(&helper_decl.decls[0].name)?.id.clone();
    let iterable = extract_single_call_arg(helper_decl.decls[0].init.as_ref()?)?;
    let item_ident = empty_single_var_ident(stmts.get(1)?)?;
    let try_stmt = stmt_as_try(stmts.get(2)?)?;
    let helper_loop = extract_iterator_helper_loop(try_stmt, &helper_ident, &item_ident)?;

    if stmts[3..].iter().any(|stmt| {
        stmt_uses_ident_key(stmt, &item_ident) || stmt_uses_ident_key(stmt, &helper_ident)
    }) {
        return None;
    }

    let for_of = build_helper_for_of(helper_loop, iterable, item_ident)?;
    Some(SequenceRewrite {
        consumed_stmts: 3,
        preserved_stmts: Vec::new(),
        for_of,
    })
}

fn try_convert_loose_iterator_sequence(stmts: &[Stmt]) -> Option<SequenceRewrite> {
    let item_ident = empty_single_var_ident(stmts.first()?)?;
    let Stmt::For(for_stmt) = stmts.get(1)? else {
        return None;
    };
    let Some(VarDeclOrExpr::VarDecl(init_decl)) = &for_stmt.init else {
        return None;
    };
    let [helper_decl] = init_decl.decls.as_slice() else {
        return None;
    };
    let helper_ident = pat_as_ident(&helper_decl.name)?.id.clone();
    let iterable = extract_single_call_arg(helper_decl.init.as_ref()?)?;
    if for_stmt.update.is_some() {
        return None;
    }
    if !is_loose_iterator_test(for_stmt.test.as_deref()?, &helper_ident, &item_ident) {
        return None;
    }
    if stmts[2..].iter().any(|stmt| {
        stmt_uses_ident_key(stmt, &item_ident) || stmt_uses_ident_key(stmt, &helper_ident)
    }) {
        return None;
    }

    let Stmt::Block(body) = &*for_stmt.body else {
        return None;
    };
    let for_of = build_helper_for_of(body.clone(), iterable, item_ident)?;
    Some(SequenceRewrite {
        consumed_stmts: 2,
        preserved_stmts: Vec::new(),
        for_of,
    })
}

fn try_convert_ts_values_sequence(stmts: &[Stmt]) -> Option<SequenceRewrite> {
    let error_ident = empty_single_var_ident(stmts.first()?)?;
    let return_ident = empty_single_var_ident(stmts.get(1)?)?;
    let try_stmt = stmt_as_try(stmts.get(2)?)?;
    let helper_loop = extract_ts_values_loop(try_stmt, &error_ident, &return_ident)?;
    if stmts[3..].iter().any(|stmt| {
        stmt_uses_ident_key(stmt, &error_ident) || stmt_uses_ident_key(stmt, &return_ident)
    }) {
        return None;
    }

    let for_of = build_helper_for_of(
        helper_loop.loop_body,
        helper_loop.iterable,
        helper_loop.result_ident,
    )?;
    Some(SequenceRewrite {
        consumed_stmts: 3,
        preserved_stmts: Vec::new(),
        for_of,
    })
}

fn try_convert_swc_iterator_sequence(stmts: &[Stmt]) -> Option<SequenceRewrite> {
    let normal_ident = single_var_ident_with_bool(stmts.first()?, true)?;
    let did_error_ident = single_var_ident_with_bool(stmts.get(1)?, false)?;
    let error_ident = empty_single_var_ident(stmts.get(2)?)?;
    let try_stmt = stmt_as_try(stmts.get(3)?)?;
    let helper_loop = extract_swc_iterator_loop(try_stmt, &normal_ident)?;

    if !swc_catch_matches(try_stmt, &did_error_ident, &error_ident) {
        return None;
    }
    if !try_stmt.finalizer.as_ref().is_some_and(|finalizer| {
        let stmt = Stmt::Block(finalizer.clone());
        stmt_uses_ident_key(&stmt, &normal_ident)
            && stmt_uses_ident_key(&stmt, &did_error_ident)
            && stmt_uses_ident_key(&stmt, &error_ident)
            && stmt_uses_ident_key(&stmt, &helper_loop.iterator_ident)
    }) {
        return None;
    }
    if stmts[4..].iter().any(|stmt| {
        stmt_uses_ident_key(stmt, &normal_ident)
            || stmt_uses_ident_key(stmt, &did_error_ident)
            || stmt_uses_ident_key(stmt, &error_ident)
    }) {
        return None;
    }

    let for_of = build_helper_for_of(
        helper_loop.loop_body,
        helper_loop.iterable,
        helper_loop.result_ident,
    )?;
    Some(SequenceRewrite {
        consumed_stmts: 4,
        preserved_stmts: Vec::new(),
        for_of,
    })
}

struct TsValuesLoop {
    iterable: Box<Expr>,
    result_ident: Ident,
    loop_body: BlockStmt,
}

struct SwcIteratorLoop {
    iterable: Box<Expr>,
    iterator_ident: Ident,
    result_ident: Ident,
    loop_body: BlockStmt,
}

fn extract_iterator_helper_loop(
    try_stmt: &TryStmt,
    helper_ident: &Ident,
    item_ident: &Ident,
) -> Option<BlockStmt> {
    let for_stmt = single_for_stmt(&try_stmt.block)?;

    let Some(VarDeclOrExpr::Expr(init)) = &for_stmt.init else {
        return None;
    };
    if !is_helper_method_call(init, helper_ident, "s") {
        return None;
    }
    if for_stmt.update.is_some() {
        return None;
    }
    let test = for_stmt.test.as_deref()?;
    if !is_iterator_helper_test(test, helper_ident, item_ident) {
        return None;
    }
    if !catch_calls_helper_error(try_stmt, helper_ident) {
        return None;
    }
    if !finally_calls_helper_method(try_stmt.finalizer.as_ref()?, helper_ident, "f") {
        return None;
    }

    let Stmt::Block(body) = &*for_stmt.body else {
        return None;
    };
    Some(body.clone())
}

fn extract_ts_values_loop(
    try_stmt: &TryStmt,
    error_ident: &Ident,
    return_ident: &Ident,
) -> Option<TsValuesLoop> {
    let for_stmt = single_for_stmt(&try_stmt.block)?;
    let Some(VarDeclOrExpr::VarDecl(init_decl)) = &for_stmt.init else {
        return None;
    };
    let [iterator_decl, result_decl] = init_decl.decls.as_slice() else {
        return None;
    };
    let iterator_ident = pat_as_ident(&iterator_decl.name)?.id.clone();
    let result_ident = pat_as_ident(&result_decl.name)?.id.clone();
    let iterable = extract_ts_values_arg(iterator_decl.init.as_ref()?)?;
    if !is_iterator_next_call(result_decl.init.as_ref()?, &iterator_ident) {
        return None;
    }
    if !is_not_done_test(for_stmt.test.as_deref()?, &result_ident) {
        return None;
    }
    if !for_stmt
        .update
        .as_deref()
        .is_some_and(|update| is_iterator_next_update(update, &result_ident, &iterator_ident))
    {
        return None;
    }
    if !ts_values_catch_matches(try_stmt, error_ident) {
        return None;
    }
    if !try_stmt.finalizer.as_ref().is_some_and(|finalizer| {
        stmt_uses_ident_key(&Stmt::Block(finalizer.clone()), return_ident)
            && stmt_uses_ident_key(&Stmt::Block(finalizer.clone()), &iterator_ident)
    }) {
        return None;
    }

    let Stmt::Block(body) = &*for_stmt.body else {
        return None;
    };
    Some(TsValuesLoop {
        iterable,
        result_ident,
        loop_body: body.clone(),
    })
}

fn extract_swc_iterator_loop(try_stmt: &TryStmt, normal_ident: &Ident) -> Option<SwcIteratorLoop> {
    let [step_decl_stmt, Stmt::For(for_stmt)] = try_stmt.block.stmts.as_slice() else {
        return None;
    };
    let result_ident = empty_single_var_ident(step_decl_stmt)?;
    let Some(VarDeclOrExpr::VarDecl(init_decl)) = &for_stmt.init else {
        return None;
    };
    let [iterator_decl] = init_decl.decls.as_slice() else {
        return None;
    };
    let iterator_ident = pat_as_ident(&iterator_decl.name)?.id.clone();
    let iterable = extract_symbol_iterator_call_obj(iterator_decl.init.as_ref()?)?;
    if !is_swc_iterator_test(
        for_stmt.test.as_deref()?,
        normal_ident,
        &result_ident,
        &iterator_ident,
    ) {
        return None;
    }
    if !for_stmt
        .update
        .as_deref()
        .is_some_and(|update| is_assign_bool(update, normal_ident, true))
    {
        return None;
    }
    let Stmt::Block(body) = &*for_stmt.body else {
        return None;
    };
    Some(SwcIteratorLoop {
        iterable,
        iterator_ident,
        result_ident,
        loop_body: body.clone(),
    })
}

fn build_helper_for_of(
    mut body: BlockStmt,
    iterable: Box<Expr>,
    item_ident: Ident,
) -> Option<ForOfStmt> {
    let mut element = extract_iterator_value_element(&body.stmts, &item_ident);
    if element.is_none() {
        element = extract_iterator_call_destructuring_element(&body.stmts, &item_ident);
    }
    if element.is_none() {
        element = extract_iterator_destructuring_decl_element(&body.stmts, &item_ident);
    }
    if element.is_none() {
        if body
            .stmts
            .iter()
            .any(|stmt| stmt_uses_ident_key_outside_value_member(stmt, &item_ident))
        {
            return None;
        }
        replace_iterator_value_refs(&mut body, &item_ident);
    }
    let (pat, bindings, kind, consumed_stmts, temp_ident) = if let Some(element) = element {
        (
            element.pat,
            element.bindings,
            element.kind,
            element.consumed_stmts,
            element.temp_ident,
        )
    } else {
        (
            Pat::Ident(BindingIdent {
                id: item_ident.clone(),
                type_ann: None,
            }),
            vec![item_ident.clone()],
            VarDeclKind::Const,
            0,
            None,
        )
    };

    let mut remaining_body = body.stmts[consumed_stmts..].to_vec();
    if consumed_stmts > 0
        && remaining_body
            .iter()
            .any(|stmt| stmt_uses_ident_key(stmt, &item_ident))
    {
        return None;
    }
    if temp_ident
        .as_ref()
        .is_some_and(|id| remaining_body.iter().any(|stmt| stmt_uses_ident(stmt, id)))
    {
        return None;
    }

    let is_reassigned = remaining_body
        .iter()
        .any(|stmt| bindings.iter().any(|id| stmt_assigns_ident(stmt, id)));
    let kind = if kind == VarDeclKind::Var {
        VarDeclKind::Var
    } else if is_reassigned {
        VarDeclKind::Let
    } else {
        VarDeclKind::Const
    };

    Some(ForOfStmt {
        span: DUMMY_SP,
        is_await: false,
        left: ForHead::VarDecl(Box::new(VarDecl {
            span: DUMMY_SP,
            ctxt: Default::default(),
            kind,
            declare: false,
            decls: vec![VarDeclarator {
                span: DUMMY_SP,
                name: pat,
                init: None,
                definite: false,
            }],
        })),
        right: iterable,
        body: Box::new(Stmt::Block(BlockStmt {
            span: body.span,
            ctxt: body.ctxt,
            stmts: std::mem::take(&mut remaining_body),
        })),
    })
}

fn extract_iterator_call_destructuring_element(
    stmts: &[Stmt],
    item_ident: &Ident,
) -> Option<LoopElement> {
    let first_decl = stmt_as_single_var_decl(stmts.first()?)?;
    let first = &first_decl.decls[0];
    let Pat::Ident(temp_binding) = &first.name else {
        return None;
    };
    if !is_destructuring_helper_call(first.init.as_ref()?, item_ident) {
        return None;
    }

    let temp_ident = &temp_binding.id;
    let mut elems = Vec::new();
    let mut bindings = Vec::new();
    let mut consumed_stmts = 1;

    for stmt in &stmts[1..] {
        let Some(decl) = stmt_as_single_var_decl(stmt) else {
            break;
        };
        let declarator = &decl.decls[0];
        let expected_index = elems.len() as f64;
        let Pat::Ident(binding) = &declarator.name else {
            break;
        };
        let Some(init) = declarator.init.as_ref() else {
            break;
        };
        if !is_numeric_index_access(init, &temp_ident.sym, expected_index) {
            break;
        }

        elems.push(Some(Pat::Ident(BindingIdent {
            id: binding.id.clone(),
            type_ann: binding.type_ann.clone(),
        })));
        bindings.push(binding.id.clone());
        consumed_stmts += 1;
    }

    if elems.is_empty() {
        return None;
    }

    Some(LoopElement {
        pat: Pat::Array(ArrayPat {
            span: DUMMY_SP,
            elems,
            optional: false,
            type_ann: None,
        }),
        bindings,
        kind: first_decl.kind,
        temp_ident: Some(temp_ident.clone()),
        consumed_stmts,
    })
}

fn extract_iterator_destructuring_decl_element(
    stmts: &[Stmt],
    item_ident: &Ident,
) -> Option<LoopElement> {
    let first_decl = stmt_as_single_var_decl(stmts.first()?)?;
    let first = &first_decl.decls[0];
    if matches!(first.name, Pat::Ident(_)) {
        return None;
    }
    let init = first.init.as_ref()?;
    if !is_value_member(init, item_ident) {
        return None;
    }
    if pat_uses_ident_key(&first.name, item_ident) {
        return None;
    }

    let mut bindings = Vec::new();
    collect_pat_bindings(&first.name, &mut bindings)?;
    if bindings.is_empty() {
        return None;
    }

    Some(LoopElement {
        pat: first.name.clone(),
        bindings,
        kind: first_decl.kind,
        temp_ident: None,
        consumed_stmts: 1,
    })
}

fn extract_iterator_value_element(stmts: &[Stmt], item_ident: &Ident) -> Option<LoopElement> {
    let first_decl = stmt_as_single_var_decl(stmts.first()?)?;
    let first = &first_decl.decls[0];
    let Pat::Ident(binding) = &first.name else {
        return None;
    };
    if !is_value_member(first.init.as_ref()?, item_ident) {
        return None;
    }

    let temp_ident = &binding.id;
    let mut elems = Vec::new();
    let mut bindings = Vec::new();
    let mut consumed_stmts = 1;

    for stmt in &stmts[1..] {
        let Some(decl) = stmt_as_single_var_decl(stmt) else {
            break;
        };
        let declarator = &decl.decls[0];
        let expected_index = elems.len() as f64;
        let Pat::Ident(binding) = &declarator.name else {
            break;
        };
        let Some(init) = declarator.init.as_ref() else {
            break;
        };
        if !is_numeric_index_access(init, &temp_ident.sym, expected_index) {
            break;
        }

        elems.push(Some(Pat::Ident(BindingIdent {
            id: binding.id.clone(),
            type_ann: binding.type_ann.clone(),
        })));
        bindings.push(binding.id.clone());
        consumed_stmts += 1;
    }

    if !elems.is_empty() {
        return Some(LoopElement {
            pat: Pat::Array(ArrayPat {
                span: DUMMY_SP,
                elems,
                optional: false,
                type_ann: None,
            }),
            bindings,
            kind: first_decl.kind,
            temp_ident: Some(temp_ident.clone()),
            consumed_stmts,
        });
    }

    Some(LoopElement {
        pat: Pat::Ident(binding.clone()),
        bindings: vec![binding.id.clone()],
        kind: first_decl.kind,
        temp_ident: None,
        consumed_stmts: 1,
    })
}

fn collect_pat_bindings(pat: &Pat, bindings: &mut Vec<Ident>) -> Option<()> {
    match pat {
        Pat::Ident(binding) => {
            bindings.push(binding.id.clone());
            Some(())
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_bindings(elem, bindings)?;
            }
            Some(())
        }
        Pat::Rest(rest) => collect_pat_bindings(&rest.arg, bindings),
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(key_value) => {
                        collect_pat_bindings(&key_value.value, bindings)?;
                    }
                    ObjectPatProp::Assign(assign) => {
                        bindings.push(assign.key.id.clone());
                    }
                    ObjectPatProp::Rest(rest) => {
                        collect_pat_bindings(&rest.arg, bindings)?;
                    }
                }
            }
            Some(())
        }
        Pat::Assign(assign) => collect_pat_bindings(&assign.left, bindings),
        Pat::Expr(_) | Pat::Invalid(_) => None,
    }
}

fn single_for_stmt(block: &BlockStmt) -> Option<&swc_core::ecma::ast::ForStmt> {
    let [Stmt::For(for_stmt)] = block.stmts.as_slice() else {
        return None;
    };
    Some(for_stmt)
}

fn empty_single_var_ident(stmt: &Stmt) -> Option<Ident> {
    let decl = stmt_as_single_var_decl(stmt)?;
    let declarator = &decl.decls[0];
    if declarator.init.is_some() {
        return None;
    }
    Some(pat_as_ident(&declarator.name)?.id.clone())
}

fn single_var_ident_with_bool(stmt: &Stmt, value: bool) -> Option<Ident> {
    let decl = stmt_as_single_var_decl(stmt)?;
    let declarator = &decl.decls[0];
    if !declarator.init.as_deref().is_some_and(
        |init| matches!(init, Expr::Lit(Lit::Bool(bool_lit)) if bool_lit.value == value),
    ) {
        return None;
    }
    Some(pat_as_ident(&declarator.name)?.id.clone())
}

fn pat_as_ident(pat: &Pat) -> Option<&BindingIdent> {
    let Pat::Ident(ident) = pat else {
        return None;
    };
    Some(ident)
}

fn stmt_as_try(stmt: &Stmt) -> Option<&TryStmt> {
    let Stmt::Try(try_stmt) = stmt else {
        return None;
    };
    Some(try_stmt)
}

fn extract_single_call_arg(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Call(CallExpr { args, .. }) = expr else {
        return None;
    };
    let [ExprOrSpread { spread: None, expr }] = args.as_slice() else {
        return None;
    };
    Some(expr.clone())
}

fn extract_ts_values_arg(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Call(CallExpr { callee, args, .. }) = expr else {
        return None;
    };
    if !callee_is_prop(callee, "__values") && !callee_is_ident(callee, "__values") {
        return None;
    }
    let [ExprOrSpread { spread: None, expr }] = args.as_slice() else {
        return None;
    };
    Some(expr.clone())
}

fn is_loose_iterator_test(expr: &Expr, helper_ident: &Ident, item_ident: &Ident) -> bool {
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = expr
    else {
        return false;
    };
    let Some(done_obj) = extract_done_obj(arg) else {
        return false;
    };
    let Expr::Assign(assign) = done_obj else {
        return false;
    };
    is_assign_ident(assign, item_ident) && is_helper_call(&assign.right, helper_ident)
}

fn is_swc_iterator_test(
    expr: &Expr,
    normal_ident: &Ident,
    result_ident: &Ident,
    iterator_ident: &Ident,
) -> bool {
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = expr
    else {
        return false;
    };
    let Expr::Assign(normal_assign) = strip_paren(arg) else {
        return false;
    };
    if !is_assign_ident(normal_assign, normal_ident) {
        return false;
    }
    let Some(done_obj) = extract_done_obj(&normal_assign.right) else {
        return false;
    };
    let Expr::Assign(next_assign) = done_obj else {
        return false;
    };
    is_assign_ident(next_assign, result_ident)
        && is_iterator_next_call(&next_assign.right, iterator_ident)
}

fn is_iterator_helper_test(expr: &Expr, helper_ident: &Ident, item_ident: &Ident) -> bool {
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = expr
    else {
        return false;
    };
    let Some(done_obj) = extract_done_obj(arg) else {
        return false;
    };
    let Expr::Assign(assign) = done_obj else {
        return false;
    };
    is_assign_ident(assign, item_ident) && is_helper_method_call(&assign.right, helper_ident, "n")
}

fn is_not_done_test(expr: &Expr, result_ident: &Ident) -> bool {
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = expr
    else {
        return false;
    };
    let Some(done_obj) = extract_done_obj(arg) else {
        return false;
    };
    is_ident_key(done_obj, result_ident)
}

fn extract_done_obj(expr: &Expr) -> Option<&Expr> {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return None;
    };
    let MemberProp::Ident(prop) = prop else {
        return None;
    };
    if prop.sym.as_ref() != "done" {
        return None;
    }
    Some(strip_paren(obj))
}

fn is_assign_ident(assign: &AssignExpr, ident: &Ident) -> bool {
    if assign.op != AssignOp::Assign {
        return false;
    }
    matches!(
        &assign.left,
        AssignTarget::Simple(SimpleAssignTarget::Ident(left)) if left.id.sym == ident.sym && left.id.ctxt == ident.ctxt
    )
}

fn is_iterator_next_call(expr: &Expr, iterator_ident: &Ident) -> bool {
    is_helper_method_call(expr, iterator_ident, "next")
}

fn is_iterator_next_update(expr: &Expr, result_ident: &Ident, iterator_ident: &Ident) -> bool {
    let Expr::Assign(assign) = expr else {
        return false;
    };
    is_assign_ident(assign, result_ident) && is_iterator_next_call(&assign.right, iterator_ident)
}

fn is_assign_bool(expr: &Expr, ident: &Ident, value: bool) -> bool {
    let Expr::Assign(assign) = expr else {
        return false;
    };
    is_assign_ident(assign, ident)
        && matches!(&*assign.right, Expr::Lit(Lit::Bool(bool_lit)) if bool_lit.value == value)
}

fn is_helper_method_call(expr: &Expr, helper_ident: &Ident, method: &str) -> bool {
    let Expr::Call(CallExpr { callee, args, .. }) = expr else {
        return false;
    };
    if !args.is_empty() {
        return false;
    }
    let Callee::Expr(callee_expr) = callee else {
        return false;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return false;
    };
    if !is_ident_key(obj, helper_ident) {
        return false;
    }
    matches!(prop, MemberProp::Ident(prop) if prop.sym.as_ref() == method)
}

fn is_helper_call(expr: &Expr, helper_ident: &Ident) -> bool {
    let Expr::Call(CallExpr { callee, args, .. }) = expr else {
        return false;
    };
    if !args.is_empty() {
        return false;
    }
    let Callee::Expr(callee_expr) = callee else {
        return false;
    };
    is_ident_key(callee_expr, helper_ident)
}

fn catch_calls_helper_error(try_stmt: &TryStmt, helper_ident: &Ident) -> bool {
    let Some(catch) = &try_stmt.handler else {
        return false;
    };
    let [Stmt::Expr(expr_stmt)] = catch.body.stmts.as_slice() else {
        return false;
    };
    let Expr::Call(CallExpr { callee, args, .. }) = &*expr_stmt.expr else {
        return false;
    };
    if args.len() != 1 {
        return false;
    }
    let Callee::Expr(callee_expr) = callee else {
        return false;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return false;
    };
    is_ident_key(obj, helper_ident)
        && matches!(prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "e")
}

fn finally_calls_helper_method(block: &BlockStmt, helper_ident: &Ident, method: &str) -> bool {
    let [Stmt::Expr(expr_stmt)] = block.stmts.as_slice() else {
        return false;
    };
    is_helper_method_call(&expr_stmt.expr, helper_ident, method)
}

fn ts_values_catch_matches(try_stmt: &TryStmt, error_ident: &Ident) -> bool {
    let Some(catch) = &try_stmt.handler else {
        return false;
    };
    let [Stmt::Expr(expr_stmt)] = catch.body.stmts.as_slice() else {
        return false;
    };
    let Expr::Assign(assign) = &*expr_stmt.expr else {
        return false;
    };
    is_assign_ident(assign, error_ident)
}

fn swc_catch_matches(try_stmt: &TryStmt, did_error_ident: &Ident, error_ident: &Ident) -> bool {
    let Some(catch) = &try_stmt.handler else {
        return false;
    };
    let [Stmt::Expr(first), Stmt::Expr(second)] = catch.body.stmts.as_slice() else {
        return false;
    };
    if !is_assign_bool(&first.expr, did_error_ident, true) {
        return false;
    }
    let Some(param) = catch.param.as_ref().and_then(pat_as_ident) else {
        return false;
    };
    let Expr::Assign(assign) = &*second.expr else {
        return false;
    };
    is_assign_ident(assign, error_ident) && is_ident_key(&assign.right, &param.id)
}

fn is_value_member(expr: &Expr, item_ident: &Ident) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    is_ident_key(obj, item_ident)
        && matches!(prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value")
}

fn pat_uses_ident_key(pat: &Pat, ident: &Ident) -> bool {
    use swc_core::ecma::visit::Visit;

    struct IdentFinder {
        ident: Ident,
        found: bool,
    }

    impl Visit for IdentFinder {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.ident.sym && ident.ctxt == self.ident.ctxt {
                self.found = true;
            }
        }
    }

    let mut finder = IdentFinder {
        ident: ident.clone(),
        found: false,
    };
    finder.visit_pat(pat);
    finder.found
}

fn is_destructuring_helper_call(expr: &Expr, item_ident: &Ident) -> bool {
    let Expr::Call(CallExpr { args, .. }) = expr else {
        return false;
    };
    let Some(ExprOrSpread { spread: None, expr }) = args.first() else {
        return false;
    };
    is_value_member(expr, item_ident)
}

fn extract_symbol_iterator_call_obj(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Call(CallExpr { callee, args, .. }) = expr else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let Callee::Expr(callee_expr) = callee else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return None;
    };
    let MemberProp::Computed(computed) = prop else {
        return None;
    };
    if !is_symbol_iterator_expr(&computed.expr) {
        return None;
    }
    Some(obj.clone())
}

fn is_symbol_iterator_expr(expr: &Expr) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    is_ident(obj, &Atom::from("Symbol"))
        && matches!(prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "iterator")
}

fn replace_iterator_value_refs(block: &mut BlockStmt, item_ident: &Ident) {
    struct Replacer {
        ident: Ident,
    }

    impl VisitMut for Replacer {
        fn visit_mut_expr(&mut self, expr: &mut Expr) {
            expr.visit_mut_children_with(self);
            if is_value_member(expr, &self.ident) {
                *expr = Expr::Ident(self.ident.clone());
            }
        }
    }

    block.visit_mut_with(&mut Replacer {
        ident: item_ident.clone(),
    });
}

fn strip_paren(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_paren(&paren.expr),
        _ => expr,
    }
}

fn callee_is_prop(callee: &Callee, prop_name: &str) -> bool {
    let Callee::Expr(callee_expr) = callee else {
        return false;
    };
    let Expr::Member(MemberExpr { prop, .. }) = &**callee_expr else {
        return false;
    };
    matches!(prop, MemberProp::Ident(prop) if prop.sym.as_ref() == prop_name)
}

fn callee_is_ident(callee: &Callee, sym: &str) -> bool {
    let Callee::Expr(callee_expr) = callee else {
        return false;
    };
    matches!(&**callee_expr, Expr::Ident(ident) if ident.sym.as_ref() == sym)
}

fn try_convert_for_of(stmt: &Stmt) -> Option<ForOfStmt> {
    let Stmt::For(for_stmt) = stmt else {
        return None;
    };

    // --- Init: `let i = 0, arr = <iterable>` ---
    let Some(swc_core::ecma::ast::VarDeclOrExpr::VarDecl(init_decl)) = &for_stmt.init else {
        return None;
    };
    if init_decl.decls.is_empty() || init_decl.decls.len() > 2 {
        return None;
    }
    let idx_decl = &init_decl.decls[0];

    // Index must be initialized to 0
    let Pat::Ident(idx_binding) = &idx_decl.name else {
        return None;
    };
    let idx_ident = &idx_binding.id;
    let Some(idx_init) = &idx_decl.init else {
        return None;
    };
    if !is_zero(idx_init) {
        return None;
    }

    // --- Test: `i < arr.length` ---
    let Some(test) = &for_stmt.test else {
        return None;
    };
    let Expr::Bin(BinExpr {
        op: BinaryOp::Lt,
        left,
        right,
        ..
    }) = &**test
    else {
        return None;
    };
    if !is_ident(left, &idx_ident.sym) {
        return None;
    }

    let IndexedIterable {
        access_obj,
        iterable,
        temp_ident,
    } = extract_indexed_iterable(init_decl, right)?;

    // --- Update: `i++` ---
    let Some(update) = &for_stmt.update else {
        return None;
    };
    let Expr::Update(UpdateExpr {
        op: UpdateOp::PlusPlus,
        arg,
        ..
    }) = &**update
    else {
        return None;
    };
    if !is_ident(arg, &idx_ident.sym) {
        return None;
    }

    // --- Body: first statement must declare the element from `arr[i]` ---
    let Stmt::Block(block) = &*for_stmt.body else {
        return None;
    };
    if block.stmts.is_empty() {
        return None;
    }
    let element = extract_loop_element(&block.stmts, &access_obj, &idx_ident.sym)?;

    // --- Safety: generated index/temp bindings must not be used in remaining body statements ---
    let remaining_body = &block.stmts[element.consumed_stmts..];
    for body_stmt in remaining_body {
        if stmt_uses_ident(body_stmt, idx_ident) {
            return None;
        }
        if temp_ident
            .as_ref()
            .is_some_and(|id| stmt_uses_ident(body_stmt, id))
        {
            return None;
        }
        if element
            .temp_ident
            .as_ref()
            .is_some_and(|id| stmt_uses_ident(body_stmt, id))
        {
            return None;
        }
    }

    // Use `let` if the element variable is reassigned in the loop body, `const` otherwise
    let elem_is_reassigned = remaining_body.iter().any(|stmt| {
        element
            .bindings
            .iter()
            .any(|id| stmt_assigns_ident(stmt, id))
    });
    let elem_kind = if element.kind == VarDeclKind::Var {
        VarDeclKind::Var
    } else if elem_is_reassigned {
        VarDeclKind::Let
    } else {
        VarDeclKind::Const
    };

    // --- Build for...of ---
    let for_of_left = ForHead::VarDecl(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: elem_kind,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: element.pat,
            init: None,
            definite: false,
        }],
    }));

    let new_body = Stmt::Block(swc_core::ecma::ast::BlockStmt {
        span: DUMMY_SP,
        ctxt: Default::default(),
        stmts: remaining_body.to_vec(),
    });

    Some(ForOfStmt {
        span: for_stmt.span,
        is_await: false,
        left: for_of_left,
        right: iterable,
        body: Box::new(new_body),
    })
}

struct IndexedIterable {
    access_obj: Box<Expr>,
    iterable: Box<Expr>,
    temp_ident: Option<Ident>,
}

struct LoopElement {
    pat: Pat,
    bindings: Vec<Ident>,
    kind: VarDeclKind,
    temp_ident: Option<Ident>,
    consumed_stmts: usize,
}

fn extract_indexed_iterable(init_decl: &VarDecl, length_expr: &Expr) -> Option<IndexedIterable> {
    let length_obj = extract_length_obj(length_expr)?;

    match init_decl.decls.as_slice() {
        // TypeScript: `let i = 0, arr = iterable; i < arr.length; i++`
        [_, arr_decl] => {
            let Pat::Ident(arr_binding) = &arr_decl.name else {
                return None;
            };
            if !is_ident(&length_obj, &arr_binding.id.sym) {
                return None;
            }
            let iterable = arr_decl.init.clone()?;
            Some(IndexedIterable {
                access_obj: Box::new(length_obj),
                iterable,
                temp_ident: Some(arr_binding.id.clone()),
            })
        }
        // Babel `iterableIsArray`: `let i = 0; i < items.length; i++`
        [idx_decl] => {
            // The direct-array form only has the index declaration in `init`.
            idx_decl.init.as_ref()?;
            Some(IndexedIterable {
                access_obj: Box::new(length_obj.clone()),
                iterable: Box::new(length_obj),
                temp_ident: None,
            })
        }
        _ => None,
    }
}

fn extract_length_obj(expr: &Expr) -> Option<Expr> {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return None;
    };
    let MemberProp::Ident(length_prop) = prop else {
        return None;
    };
    if length_prop.sym.as_ref() != "length" {
        return None;
    }
    Some(*obj.clone())
}

fn extract_loop_element(stmts: &[Stmt], access_obj: &Expr, idx_sym: &Atom) -> Option<LoopElement> {
    let first_decl = stmt_as_single_var_decl(stmts.first()?)?;
    let first = &first_decl.decls[0];
    let Pat::Ident(temp_binding) = &first.name else {
        return None;
    };
    let temp_ident = &temp_binding.id;
    let first_init = first.init.as_ref()?;
    if !is_index_access(first_init, access_obj, idx_sym) {
        return None;
    }

    let mut elems = Vec::new();
    let mut bindings = Vec::new();
    let mut consumed_stmts = 1;

    for stmt in &stmts[1..] {
        let Some(decl) = stmt_as_single_var_decl(stmt) else {
            break;
        };
        let declarator = &decl.decls[0];
        let expected_index = elems.len() as f64;
        let Pat::Ident(binding) = &declarator.name else {
            break;
        };
        let Some(init) = declarator.init.as_ref() else {
            break;
        };
        if !is_numeric_index_access(init, &temp_ident.sym, expected_index) {
            break;
        }

        elems.push(Some(Pat::Ident(BindingIdent {
            id: binding.id.clone(),
            type_ann: binding.type_ann.clone(),
        })));
        bindings.push(binding.id.clone());
        consumed_stmts += 1;
    }

    if elems.is_empty() {
        return Some(LoopElement {
            pat: Pat::Ident(temp_binding.clone()),
            bindings: vec![temp_binding.id.clone()],
            kind: first_decl.kind,
            temp_ident: None,
            consumed_stmts,
        });
    }

    Some(LoopElement {
        pat: Pat::Array(ArrayPat {
            span: DUMMY_SP,
            elems,
            optional: false,
            type_ann: None,
        }),
        bindings,
        kind: first_decl.kind,
        temp_ident: Some(temp_ident.clone()),
        consumed_stmts,
    })
}

fn stmt_as_single_var_decl(stmt: &Stmt) -> Option<&VarDecl> {
    let Stmt::Decl(Decl::Var(decl)) = stmt else {
        return None;
    };
    (decl.decls.len() == 1).then_some(decl)
}

fn is_index_access(expr: &Expr, obj_expr: &Expr, idx_sym: &Atom) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    if !same_ident_expr(obj, obj_expr) {
        return false;
    }
    let MemberProp::Computed(computed) = prop else {
        return false;
    };
    is_ident(&computed.expr, idx_sym)
}

fn is_numeric_index_access(expr: &Expr, obj_sym: &Atom, index: f64) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    if !is_ident(obj, obj_sym) {
        return false;
    }
    let MemberProp::Computed(computed) = prop else {
        return false;
    };
    matches!(&*computed.expr, Expr::Lit(Lit::Num(num)) if num.value == index)
}

fn is_zero(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) if n.value == 0.0)
}

fn is_ident(expr: &Expr, sym: &Atom) -> bool {
    matches!(expr, Expr::Ident(id) if &id.sym == sym)
}

fn is_ident_key(expr: &Expr, ident: &Ident) -> bool {
    matches!(expr, Expr::Ident(id) if id.sym == ident.sym && id.ctxt == ident.ctxt)
}

fn same_ident_expr(left: &Expr, right: &Expr) -> bool {
    match (left, right) {
        (Expr::Ident(left), Expr::Ident(right)) => left.sym == right.sym && left.ctxt == right.ctxt,
        _ => false,
    }
}

/// Check if a statement assigns to a specific binding (by sym + ctxt).
fn stmt_assigns_ident(stmt: &Stmt, target: &Ident) -> bool {
    use swc_core::ecma::ast::{AssignTarget, SimpleAssignTarget};
    use swc_core::ecma::visit::Visit;

    struct AssignFinder {
        sym: Atom,
        ctxt: SyntaxContext,
        found: bool,
    }

    impl Visit for AssignFinder {
        fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
            if let AssignTarget::Simple(SimpleAssignTarget::Ident(id)) = &assign.left {
                if id.sym == self.sym && id.ctxt == self.ctxt {
                    self.found = true;
                }
            }
        }

        fn visit_update_expr(&mut self, update: &UpdateExpr) {
            if let Expr::Ident(id) = &*update.arg {
                if id.sym == self.sym && id.ctxt == self.ctxt {
                    self.found = true;
                }
            }
        }
    }

    let mut finder = AssignFinder {
        sym: target.sym.clone(),
        ctxt: target.ctxt,
        found: false,
    };
    finder.visit_stmt(stmt);
    finder.found
}

/// Check if a statement references a specific binding (by sym + ctxt).
fn stmt_uses_ident(stmt: &Stmt, target: &Ident) -> bool {
    use swc_core::ecma::visit::Visit;

    struct IdentFinder {
        sym: Atom,
        ctxt: SyntaxContext,
        found: bool,
    }

    impl Visit for IdentFinder {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.sym && ident.ctxt == self.ctxt {
                self.found = true;
            }
        }
    }

    let mut finder = IdentFinder {
        sym: target.sym.clone(),
        ctxt: target.ctxt,
        found: false,
    };
    finder.visit_stmt(stmt);
    finder.found
}

/// Check if a statement references the exact identifier binding.
fn stmt_uses_ident_key(stmt: &Stmt, ident: &Ident) -> bool {
    use swc_core::ecma::visit::Visit;

    struct IdentFinder {
        sym: Atom,
        ctxt: SyntaxContext,
        found: bool,
    }

    impl Visit for IdentFinder {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.sym && ident.ctxt == self.ctxt {
                self.found = true;
            }
        }
    }

    let mut finder = IdentFinder {
        sym: ident.sym.clone(),
        ctxt: ident.ctxt,
        found: false,
    };
    finder.visit_stmt(stmt);
    finder.found
}

/// Check if a statement references the iterator result binding anywhere except
/// as the object in `result.value`.
fn stmt_uses_ident_key_outside_value_member(stmt: &Stmt, ident: &Ident) -> bool {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct IdentFinder {
        sym: Atom,
        ctxt: SyntaxContext,
        found: bool,
    }

    impl Visit for IdentFinder {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.sym && ident.ctxt == self.ctxt {
                self.found = true;
            }
        }

        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if let Expr::Ident(obj) = &*member.obj {
                if obj.sym == self.sym
                    && obj.ctxt == self.ctxt
                    && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value")
                {
                    return;
                }
            }
            member.visit_children_with(self);
        }
    }

    let mut finder = IdentFinder {
        sym: ident.sym.clone(),
        ctxt: ident.ctxt,
        found: false,
    };
    finder.visit_stmt(stmt);
    finder.found
}
