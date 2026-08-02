use super::*;

pub(in crate::vue_recovery) fn render_context_param(render: RenderSource<'_>) -> Option<Atom> {
    match render {
        RenderSource::Function { render, .. } => render
            .function
            .params
            .first()
            .and_then(param_binding_ident)
            .map(|ident| ident.sym.clone()),
        RenderSource::SetupArrow { render, .. } => {
            render.params.first().and_then(|param| match param {
                Pat::Ident(binding) => Some(binding.id.sym.clone()),
                _ => None,
            })
        }
    }
}

pub(in crate::vue_recovery) fn render_setup_context_param(
    render: RenderSource<'_>,
) -> Option<Atom> {
    match render {
        RenderSource::Function { render, .. } => render
            .function
            .params
            .get(3)
            .and_then(param_binding_ident)
            .map(|ident| ident.sym.clone()),
        RenderSource::SetupArrow { .. } => None,
    }
}

pub(in crate::vue_recovery) fn render_props_context_param(
    render: RenderSource<'_>,
) -> Option<Atom> {
    match render {
        RenderSource::Function { render, .. } => render
            .function
            .params
            .get(2)
            .and_then(param_binding_ident)
            .map(|ident| ident.sym.clone()),
        RenderSource::SetupArrow { .. } => None,
    }
}

pub(super) fn render_stmts(render: RenderSource<'_>) -> Option<&[Stmt]> {
    match render {
        RenderSource::Function { render, .. } => render
            .function
            .body
            .as_ref()
            .map(|body| body.stmts.as_slice()),
        RenderSource::SetupArrow { render, .. } => match render.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(block) => Some(block.stmts.as_slice()),
            BlockStmtOrExpr::Expr(_) => None,
        },
    }
}

pub(super) fn refresh_setup_value_binding_sources(ctx: &mut VueRecoveryContext) -> Result<()> {
    let bindings = ctx.bindings.values.clone();
    for (binding, value) in bindings {
        let Some(expr) = value.expr else {
            continue;
        };
        let value = clean_expr(&print_expr(&expr, ctx)?, ctx);
        if let Some(binding) = ctx.bindings.values.get_mut(&binding) {
            binding.value = value;
        }
    }
    Ok(())
}

pub(super) fn computed_value_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueSetupValueBinding>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return Ok(None);
    };
    if !is_computed_call(call, ctx) {
        return Ok(None);
    }
    let Some(arg) = call.args.first() else {
        return Ok(None);
    };
    let Some(binding) = computed_getter_expr(arg.expr.as_ref(), ctx)? else {
        return Ok(None);
    };
    if should_inline_computed_template_binding(&binding) {
        Ok(Some(binding))
    } else {
        Ok(None)
    }
}

fn should_inline_computed_template_binding(binding: &VueSetupValueBinding) -> bool {
    !computed_value_contains_block_function(&binding.value)
        && !should_preserve_long_computed_template_binding(&binding.value)
}

fn computed_value_contains_block_function(value: &str) -> bool {
    value.contains("function") || value_contains_block_arrow(value)
}

fn should_preserve_long_computed_template_binding(value: &str) -> bool {
    let mut value = value.trim();
    while let Some(inner) = value.strip_prefix('(') {
        value = inner.trim_start();
    }
    // Keep class/style-friendly literal values inline; preserve long computed
    // expressions where a named binding is usually easier to read.
    value.len() > MAX_INLINE_COMPUTED_TEMPLATE_BINDING_LEN
        && !value.starts_with('[')
        && !value.starts_with('{')
}

fn value_contains_block_arrow(value: &str) -> bool {
    let mut cursor = 0;
    while let Some(relative_arrow) = value[cursor..].find("=>") {
        let arrow = cursor + relative_arrow + "=>".len();
        let rest = &value[arrow..];
        let body = rest.trim_start();
        if body.starts_with('{') {
            return true;
        }
        cursor = arrow;
    }
    false
}

pub(super) fn computed_script_setup_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<(String, HashSet<Atom>)>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return Ok(None);
    };
    let Some(arg) = call.args.first() else {
        return Ok(None);
    };
    if !is_computed_script_setup_call(call, arg.expr.as_ref(), ctx) {
        return Ok(None);
    }
    let getter = computed_script_setup_getter(arg.expr.as_ref(), ctx)?;
    let import_refs = script_import_refs(arg.expr.as_ref(), &ctx.script_imports);
    Ok(Some((format!("computed({getter})"), import_refs)))
}

fn computed_script_setup_getter(expr: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
    let getter = clean_expr(&print_expr(expr, ctx)?, ctx);
    if arrow_returns_object_expr(expr) {
        Ok(wrap_arrow_object_return(&getter))
    } else {
        Ok(getter)
    }
}

fn arrow_returns_object_expr(expr: &Expr) -> bool {
    let Expr::Arrow(arrow) = unwrap_paren_expr(expr) else {
        return false;
    };
    matches!(
        arrow.body.as_ref(),
        BlockStmtOrExpr::Expr(expr) if matches!(unwrap_paren_expr(expr.as_ref()), Expr::Object(_))
    )
}

fn wrap_arrow_object_return(getter: &str) -> String {
    let Some(arrow_index) = getter.find("=>") else {
        return getter.to_string();
    };
    let body_start = arrow_index + "=>".len();
    let leading_ws = getter[body_start..]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .map(char::len_utf8)
        .sum::<usize>();
    let object_start = body_start + leading_ws;
    if !getter[object_start..].starts_with('{') {
        return getter.to_string();
    }

    let mut output = String::with_capacity(getter.len() + 2);
    output.push_str(&getter[..object_start]);
    output.push('(');
    output.push_str(&getter[object_start..]);
    output.push(')');
    output
}

fn is_computed_script_setup_call(call: &CallExpr, getter: &Expr, ctx: &VueRecoveryContext) -> bool {
    let is_getter = matches!(unwrap_paren_expr(getter), Expr::Arrow(_) | Expr::Fn(_));
    if !is_getter {
        return false;
    }
    helper_name(&call.callee, ctx) == Some(VueHelper::Computed)
        || call_callee_ident(call).is_some_and(|callee| {
            ctx.vue_helper_candidates.contains(&callee.sym) && ctx.resolves_to_import(callee)
        })
}

fn script_import_refs(expr: &Expr, imports: &HashMap<Atom, VueScriptImport>) -> HashSet<Atom> {
    let declared = declared_binding_idents(expr);
    let mut collector = ScriptImportRefCollector {
        imports,
        declared: &declared,
        refs: HashSet::new(),
    };
    expr.visit_with(&mut collector);
    collector.refs
}

pub(super) fn stmt_import_refs(
    stmt: &Stmt,
    imports: &HashMap<Atom, VueScriptImport>,
) -> HashSet<Atom> {
    let declared = declared_binding_idents(stmt);
    let mut collector = ScriptImportRefCollector {
        imports,
        declared: &declared,
        refs: HashSet::new(),
    };
    stmt.visit_with(&mut collector);
    collector.refs
}

pub(in crate::vue_recovery) fn stmt_ident_refs(stmt: &Stmt) -> HashSet<Atom> {
    let declared = declared_binding_idents(stmt);
    let mut collector = IdentRefCollector {
        declared: &declared,
        refs: HashSet::new(),
    };
    stmt.visit_with(&mut collector);
    collector.refs
}

fn expr_ident_refs(expr: &Expr) -> HashSet<Atom> {
    let declared = declared_binding_idents(expr);
    let mut collector = IdentRefCollector {
        declared: &declared,
        refs: HashSet::new(),
    };
    expr.visit_with(&mut collector);
    collector.refs
}

/// Collect the `(name, SyntaxContext)` of every binding *declared* within a
/// subtree: `var`/`let`/`const` names, function/arrow/method params, catch
/// params, and `fn`/`class` declaration names. Assignment targets (`x = ...`) are
/// references to existing bindings, not declarations, and are deliberately
/// excluded — the old `ScopeStack` collectors special-cased this via
/// `visit_assign_expr_refs`; under resolver it is free because the target and its
/// declaration share one binding identity.
///
/// The key is the `(name, ctxt)` pair, not `ctxt` alone: `resolver()` assigns one
/// context per *scope*, so every binding declared in the same scope shares a
/// context. Keying on the pair distinguishes sibling bindings — the same binding
/// identity used elsewhere in recovery.
fn declared_binding_idents<N>(node: &N) -> HashSet<(Atom, SyntaxContext)>
where
    N: VisitWith<DeclaredBindingIdents>,
{
    let mut collector = DeclaredBindingIdents {
        idents: HashSet::new(),
    };
    node.visit_with(&mut collector);
    collector.idents
}

pub(super) struct DeclaredBindingIdents {
    pub(super) idents: HashSet<(Atom, SyntaxContext)>,
}

impl DeclaredBindingIdents {
    fn record_pat(&mut self, pat: &Pat) {
        collect_pat_binding_idents(pat, &mut self.idents);
    }
}

impl Visit for DeclaredBindingIdents {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        self.record_pat(&declarator.name);
        declarator.visit_children_with(self);
    }

    fn visit_param(&mut self, param: &Param) {
        self.record_pat(&param.pat);
        param.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        for param in &arrow.params {
            self.record_pat(param);
        }
        arrow.visit_children_with(self);
    }

    fn visit_catch_clause(&mut self, catch: &CatchClause) {
        if let Some(param) = &catch.param {
            self.record_pat(param);
        }
        catch.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.idents
            .insert((function.ident.sym.clone(), function.ident.ctxt));
        function.visit_children_with(self);
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.idents
            .insert((class.ident.sym.clone(), class.ident.ctxt));
        class.visit_children_with(self);
    }
}

/// Collects identifier references that are *free* in the walked subtree — those
/// whose `(name, ctxt)` binding identity is not among [`declared_binding_idents`].
/// Runs on resolver-processed ASTs, so this is exactly the shadow-safe reference
/// set the old `ScopeStack` collector computed, without the declare-as-you-go
/// bookkeeping.
pub(super) struct IdentRefCollector<'a> {
    pub(super) declared: &'a HashSet<(Atom, SyntaxContext)>,
    pub(super) refs: HashSet<Atom>,
}

impl Visit for IdentRefCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if !self.declared.contains(&(ident.sym.clone(), ident.ctxt)) {
            self.refs.insert(ident.sym.clone());
        }
    }

    fn visit_prop_name(&mut self, prop: &PropName) {
        if let PropName::Computed(computed) = prop {
            computed.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            computed.visit_with(self);
        }
    }
}

/// Like [`IdentRefCollector`] but limited to references that resolve to a script
/// import binding.
struct ScriptImportRefCollector<'a> {
    imports: &'a HashMap<Atom, VueScriptImport>,
    declared: &'a HashSet<(Atom, SyntaxContext)>,
    refs: HashSet<Atom>,
}

impl Visit for ScriptImportRefCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if self.imports.contains_key(&ident.sym)
            && !self.declared.contains(&(ident.sym.clone(), ident.ctxt))
        {
            self.refs.insert(ident.sym.clone());
        }
    }

    fn visit_prop_name(&mut self, prop: &PropName) {
        if let PropName::Computed(computed) = prop {
            computed.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            computed.visit_with(self);
        }
    }
}

fn is_computed_call(call: &CallExpr, ctx: &VueRecoveryContext) -> bool {
    if helper_name(&call.callee, ctx) == Some(VueHelper::Computed) {
        return true;
    }
    call_callee_ident(call).is_some_and(|callee| {
        ctx.vue_helper_candidates.contains(&callee.sym) && ctx.resolves_to_import(callee)
    })
}

fn computed_getter_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueSetupValueBinding>> {
    let Expr::Arrow(arrow) = unwrap_paren_expr(expr) else {
        return Ok(None);
    };
    match arrow.body.as_ref() {
        BlockStmtOrExpr::Expr(expr) => Ok(Some(VueSetupValueBinding {
            value: clean_expr(&print_expr(expr.as_ref(), ctx)?, ctx),
            expr: Some(*expr.clone()),
        })),
        BlockStmtOrExpr::BlockStmt(block) => computed_block_value_expr(&block.stmts, ctx),
    }
}

fn computed_block_value_expr(
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> Result<Option<VueSetupValueBinding>> {
    if let Some(expr) = computed_if_return_chain_expr(stmts, ctx)? {
        return Ok(Some(VueSetupValueBinding {
            value: expr,
            expr: None,
        }));
    }

    let Some((return_index, expr)) = computed_final_return_expr(stmts) else {
        return Ok(None);
    };
    let prior_stmts = &stmts[..return_index];
    if let Some(expr) = computed_array_push_expr(prior_stmts, expr, ctx)? {
        return Ok(Some(expr));
    }
    if !computed_prior_stmts_are_inlineable(prior_stmts, ctx) {
        return Ok(None);
    }
    let local_exprs = computed_block_local_exprs(prior_stmts);
    let mutated_locals = computed_mutated_local_bindings(prior_stmts, &local_exprs);
    if computed_local_ref_counts(expr, &mutated_locals)
        .values()
        .any(|count| *count > 0)
    {
        return Ok(None);
    }
    let expr = inline_computed_block_locals(expr, prior_stmts);
    let local_exprs = computed_block_local_exprs(prior_stmts);
    if computed_local_ref_counts(&expr, &local_exprs)
        .values()
        .any(|count| *count > 0)
    {
        return Ok(None);
    }
    let expr = inline_computed_setup_prop_aliases(&expr, &stmts[..return_index], ctx);
    Ok(Some(VueSetupValueBinding {
        value: clean_expr(&print_expr(&expr, ctx)?, ctx),
        expr: Some(expr),
    }))
}

fn computed_prior_stmts_are_inlineable(stmts: &[Stmt], ctx: &VueRecoveryContext) -> bool {
    stmts.iter().all(|stmt| match stmt {
        Stmt::Decl(Decl::Var(var)) => {
            if var.kind != VarDeclKind::Const || var.decls.is_empty() {
                return false;
            }
            var.decls.iter().all(|decl| {
                decl.init.is_some()
                    && (matches!(decl.name, Pat::Ident(_))
                        || matches!(decl.name, Pat::Object(_))
                            && decl
                                .init
                                .as_deref()
                                .is_some_and(|init| is_setup_props_alias(init, ctx)))
            })
        }
        _ => false,
    })
}

fn computed_array_push_expr(
    stmts: &[Stmt],
    return_expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueSetupValueBinding>> {
    let Expr::Ident(return_ident) = unwrap_paren_expr(return_expr) else {
        return Ok(None);
    };
    let Some((array_name, push_stmts)) = computed_array_builder_binding(stmts) else {
        return Ok(None);
    };
    if return_ident.sym != array_name {
        return Ok(None);
    }
    let Some(elems) = computed_array_push_elements(push_stmts, &array_name) else {
        return Ok(None);
    };
    let expr = Expr::Array(ArrayLit {
        span: DUMMY_SP,
        elems: elems.into_iter().map(Some).collect(),
    });
    let expr = inline_computed_setup_prop_aliases(&expr, stmts, ctx);

    Ok(Some(VueSetupValueBinding {
        value: clean_expr(&print_expr(&expr, ctx)?, ctx),
        expr: Some(expr),
    }))
}

fn computed_array_builder_binding(stmts: &[Stmt]) -> Option<(Atom, &[Stmt])> {
    let [first, rest @ ..] = stmts else {
        return None;
    };
    let Stmt::Decl(Decl::Var(var)) = first else {
        return None;
    };
    if var.kind != VarDeclKind::Const {
        return None;
    }
    let [decl] = var.decls.as_slice() else {
        return None;
    };
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let init = decl.init.as_deref()?;
    if !is_empty_array_expr(init) {
        return None;
    }
    Some((binding.id.sym.clone(), rest))
}

fn is_empty_array_expr(expr: &Expr) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Array(array) => array.elems.is_empty(),
        _ => false,
    }
}

fn computed_array_push_elements(stmts: &[Stmt], array_name: &Atom) -> Option<Vec<ExprOrSpread>> {
    let mut elems = Vec::new();
    for stmt in stmts {
        elems.extend(computed_array_push_stmt_elements(stmt, array_name)?);
    }
    Some(elems)
}

fn computed_array_push_stmt_elements(stmt: &Stmt, array_name: &Atom) -> Option<Vec<ExprOrSpread>> {
    if let Some(expr) = computed_array_push_arg(stmt, array_name) {
        return Some(vec![ExprOrSpread {
            spread: None,
            expr: Box::new(expr.clone()),
        }]);
    }

    let Stmt::If(if_stmt) = stmt else {
        return None;
    };
    Some(vec![ExprOrSpread {
        spread: Some(DUMMY_SP),
        expr: Box::new(Expr::Paren(ParenExpr {
            span: DUMMY_SP,
            expr: Box::new(Expr::Cond(CondExpr {
                span: DUMMY_SP,
                test: if_stmt.test.clone(),
                cons: Box::new(array_expr_from_push_branch(&if_stmt.cons, array_name)?),
                alt: Box::new(if_stmt.alt.as_deref().map_or_else(
                    || Some(empty_array_expr()),
                    |alt| array_expr_from_push_branch(alt, array_name),
                )?),
            })),
        })),
    }])
}

fn computed_array_push_arg<'a>(stmt: &'a Stmt, array_name: &Atom) -> Option<&'a Expr> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Call(call) = unwrap_paren_expr(expr_stmt.expr.as_ref()) else {
        return None;
    };
    if call.args.len() != 1 || call.args.first()?.spread.is_some() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = unwrap_paren_expr(callee.as_ref()) else {
        return None;
    };
    if !matches!(member.obj.as_ref(), Expr::Ident(object) if object.sym == *array_name) {
        return None;
    }
    if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "push") {
        return None;
    }
    call.args.first().map(|arg| arg.expr.as_ref())
}

fn array_expr_from_push_branch(stmt: &Stmt, array_name: &Atom) -> Option<Expr> {
    let elems = match stmt {
        Stmt::Block(block) => computed_array_push_elements(&block.stmts, array_name)?,
        stmt => computed_array_push_stmt_elements(stmt, array_name)?,
    };
    Some(Expr::Array(ArrayLit {
        span: DUMMY_SP,
        elems: elems.into_iter().map(Some).collect(),
    }))
}

fn empty_array_expr() -> Expr {
    Expr::Array(ArrayLit {
        span: DUMMY_SP,
        elems: Vec::new(),
    })
}

fn computed_final_return_expr(stmts: &[Stmt]) -> Option<(usize, &Expr)> {
    stmts
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, stmt)| match stmt {
            Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            }) => Some((index, expr.as_ref())),
            _ => None,
        })
}

fn inline_computed_block_locals(expr: &Expr, stmts: &[Stmt]) -> Expr {
    let mut locals = computed_block_local_exprs(stmts);
    if locals.is_empty() {
        return expr.clone();
    }

    let mut expr = expr.clone();
    while !locals.is_empty() {
        let counts = computed_local_ref_counts(&expr, &locals);
        let inline_bindings = locals
            .iter()
            .filter(|(name, expr)| {
                counts.get(*name).copied().unwrap_or_default() == 1
                    && computed_local_ref_counts(expr, &locals)
                        .values()
                        .all(|count| *count == 0)
            })
            .map(|(name, expr)| (name.clone(), expr.clone()))
            .collect::<HashMap<_, _>>();
        if inline_bindings.is_empty() {
            break;
        }
        for name in inline_bindings.keys() {
            locals.remove(name);
        }
        expr.visit_mut_with(&mut ComputedLocalInliner::new(inline_bindings));
    }

    expr
}

fn inline_computed_setup_prop_aliases(
    expr: &Expr,
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> Expr {
    let aliases = computed_setup_prop_alias_exprs(stmts, ctx);
    if aliases.is_empty() {
        return expr.clone();
    }

    inline_computed_alias_expr(expr, &aliases)
}

fn computed_setup_prop_alias_exprs(
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> HashMap<Atom, Expr> {
    let mut aliases = HashMap::new();
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        if var.kind != VarDeclKind::Const {
            continue;
        }
        for decl in &var.decls {
            let Pat::Object(object) = &decl.name else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            if !is_setup_props_alias(init, ctx) {
                continue;
            }
            collect_computed_setup_prop_aliases(object, &mut aliases);
        }
    }
    aliases
}

fn collect_computed_setup_prop_alias_var(
    var: &VarDecl,
    ctx: &VueRecoveryContext,
    aliases: &mut HashMap<Atom, Expr>,
) -> bool {
    if var.kind != VarDeclKind::Const || var.decls.is_empty() {
        return false;
    }

    let mut next_aliases = HashMap::new();
    for decl in &var.decls {
        let Pat::Object(object) = &decl.name else {
            return false;
        };
        let Some(init) = decl.init.as_deref() else {
            return false;
        };
        if !is_setup_props_alias(init, ctx) {
            return false;
        }
        if !collect_computed_setup_prop_aliases(object, &mut next_aliases) {
            return false;
        }
    }

    aliases.extend(next_aliases);
    true
}

fn collect_computed_setup_prop_aliases(
    object: &ObjectPat,
    aliases: &mut HashMap<Atom, Expr>,
) -> bool {
    let mut next_aliases = HashMap::new();
    for prop in &object.props {
        match prop {
            ObjectPatProp::KeyValue(key_value) => {
                let Some(name) =
                    prop_name(&key_value.key).filter(|name| is_valid_identifier_name(name))
                else {
                    return false;
                };
                let Some(binding) = ident_binding_from_pat(key_value.value.as_ref()) else {
                    return false;
                };
                next_aliases.insert(
                    binding.sym.clone(),
                    Expr::Ident(Ident::new(name.into(), DUMMY_SP, Default::default())),
                );
            }
            ObjectPatProp::Assign(assign) => {
                let name = assign.key.sym.as_ref();
                if !is_valid_identifier_name(name) {
                    return false;
                }
                next_aliases.insert(
                    assign.key.sym.clone(),
                    Expr::Ident(Ident::new(
                        assign.key.sym.clone(),
                        DUMMY_SP,
                        Default::default(),
                    )),
                );
            }
            ObjectPatProp::Rest(_) => return false,
        }
    }
    if next_aliases.is_empty() {
        return false;
    }

    aliases.extend(next_aliases);
    true
}

fn ident_binding_from_pat(pat: &Pat) -> Option<&Ident> {
    match pat {
        Pat::Ident(binding) => Some(&binding.id),
        Pat::Assign(assign) => ident_binding_from_pat(assign.left.as_ref()),
        _ => None,
    }
}

fn computed_block_local_exprs(stmts: &[Stmt]) -> HashMap<Atom, Expr> {
    let mut locals = HashMap::new();
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        if var.kind != VarDeclKind::Const {
            continue;
        }
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            locals.insert(binding.id.sym.clone(), init.clone());
        }
    }
    locals
}

fn computed_mutated_local_bindings(
    stmts: &[Stmt],
    locals: &HashMap<Atom, Expr>,
) -> HashMap<Atom, Expr> {
    if locals.is_empty() {
        return HashMap::new();
    }

    let mut detector = ComputedLocalMutationDetector::new(locals.keys().cloned().collect());
    for stmt in stmts {
        stmt.visit_with(&mut detector);
    }
    let mutated = detector.finish();
    locals
        .iter()
        .filter_map(|(name, expr)| {
            mutated
                .contains(name)
                .then_some((name.clone(), expr.clone()))
        })
        .collect()
}

fn computed_local_ref_counts(expr: &Expr, locals: &HashMap<Atom, Expr>) -> HashMap<Atom, usize> {
    let mut counter = ComputedLocalRefCounter::new(locals.keys().cloned().collect());
    expr.visit_with(&mut counter);
    counter.finish()
}

struct ComputedLocalMutationDetector {
    bindings: Vec<Atom>,
    shadow_depths: Vec<usize>,
    mutated: HashSet<Atom>,
}

impl ComputedLocalMutationDetector {
    fn new(mut bindings: Vec<Atom>) -> Self {
        bindings.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
        bindings.dedup();
        let shadow_depths = vec![0; bindings.len()];
        Self {
            bindings,
            shadow_depths,
            mutated: HashSet::new(),
        }
    }

    fn finish(self) -> HashSet<Atom> {
        self.mutated
    }

    fn active_index(&self, name: &str) -> Option<usize> {
        self.bindings
            .iter()
            .zip(self.shadow_depths.iter())
            .position(|(binding, shadow_depth)| binding.as_ref() == name && *shadow_depth == 0)
    }

    fn mark_name(&mut self, name: &str) {
        if let Some(index) = self.active_index(name) {
            self.mutated.insert(self.bindings[index].clone());
        }
    }

    fn mark_member_object(&mut self, member: &MemberExpr) {
        if let Expr::Ident(object) = member.obj.as_ref() {
            self.mark_name(object.sym.as_ref());
        }
    }

    fn shadowing_indices(&self, params: &[&Pat]) -> Vec<usize> {
        let mut param_bindings = HashSet::new();
        for param in params {
            collect_pat_bindings(param, &mut param_bindings);
        }
        self.bindings
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| param_bindings.contains(binding).then_some(index))
            .collect()
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }
}

impl Visit for ComputedLocalMutationDetector {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                self.mark_name(binding.id.sym.as_ref());
            }
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                self.mark_member_object(member);
            }
            _ => {}
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        match update.arg.as_ref() {
            Expr::Ident(ident) => self.mark_name(ident.sym.as_ref()),
            Expr::Member(member) => self.mark_member_object(member),
            _ => {}
        }
        update.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(member) = callee.as_ref() {
                self.mark_member_object(member);
            }
        }
        call.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        let params = arrow.params.iter().collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        arrow.body.visit_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        if let Some(body) = function.body.as_ref() {
            body.visit_with(self);
        }
        self.exit_shadowed(&shadowed);
    }
}

struct ComputedLocalRefCounter {
    bindings: Vec<Atom>,
    shadow_depths: Vec<usize>,
    counts: Vec<usize>,
}

impl ComputedLocalRefCounter {
    fn new(mut bindings: Vec<Atom>) -> Self {
        bindings.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
        bindings.dedup();
        let shadow_depths = vec![0; bindings.len()];
        let counts = vec![0; bindings.len()];
        Self {
            bindings,
            shadow_depths,
            counts,
        }
    }

    fn finish(self) -> HashMap<Atom, usize> {
        self.bindings.into_iter().zip(self.counts).collect()
    }

    fn active_index(&self, name: &str) -> Option<usize> {
        self.bindings
            .iter()
            .zip(self.shadow_depths.iter())
            .position(|(binding, shadow_depth)| binding.as_ref() == name && *shadow_depth == 0)
    }

    fn shadowing_indices(&self, params: &[&Pat]) -> Vec<usize> {
        let mut param_bindings = HashSet::new();
        for param in params {
            collect_pat_bindings(param, &mut param_bindings);
        }
        self.bindings
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| param_bindings.contains(binding).then_some(index))
            .collect()
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }
}

impl Visit for ComputedLocalRefCounter {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Ident(ident) = expr {
            if let Some(index) = self.active_index(ident.sym.as_ref()) {
                self.counts[index] += 1;
                return;
            }
        }
        expr.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        let params = arrow.params.iter().collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        arrow.body.visit_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        if let Some(body) = function.body.as_ref() {
            body.visit_with(self);
        }
        self.exit_shadowed(&shadowed);
    }
}

struct ComputedLocalInliner {
    bindings: Vec<(Atom, Expr)>,
    replacement_refs: Vec<HashSet<Atom>>,
    shadow_depths: Vec<usize>,
    capture_depths: Vec<usize>,
}

impl ComputedLocalInliner {
    fn new(mut bindings: HashMap<Atom, Expr>) -> Self {
        let mut bindings = bindings.drain().collect::<Vec<_>>();
        bindings.sort_by(|(left, _), (right, _)| left.as_ref().cmp(right.as_ref()));
        let replacement_refs = bindings
            .iter()
            .map(|(_, expr)| expr_ident_refs(expr))
            .collect::<Vec<_>>();
        let shadow_depths = vec![0; bindings.len()];
        let capture_depths = vec![0; bindings.len()];
        Self {
            bindings,
            replacement_refs,
            shadow_depths,
            capture_depths,
        }
    }

    fn active_index(&self, name: &str) -> Option<usize> {
        self.bindings
            .iter()
            .zip(self.shadow_depths.iter())
            .zip(self.capture_depths.iter())
            .position(|(((binding, _), shadow_depth), capture_depth)| {
                binding.as_ref() == name && *shadow_depth == 0 && *capture_depth == 0
            })
    }

    fn shadowing_indices(&self, scope_bindings: &HashSet<Atom>) -> Vec<usize> {
        self.bindings
            .iter()
            .enumerate()
            .filter_map(|(index, (binding, _))| scope_bindings.contains(binding).then_some(index))
            .collect()
    }

    fn capture_indices(&self, scope_bindings: &HashSet<Atom>) -> Vec<usize> {
        self.replacement_refs
            .iter()
            .enumerate()
            .filter_map(|(index, refs)| {
                refs.iter()
                    .any(|name| scope_bindings.contains(name))
                    .then_some(index)
            })
            .collect()
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }

    fn enter_captured(&mut self, indices: &[usize]) {
        for index in indices {
            self.capture_depths[*index] += 1;
        }
    }

    fn exit_captured(&mut self, indices: &[usize]) {
        for index in indices {
            self.capture_depths[*index] -= 1;
        }
    }

    fn enter_scope(&mut self, scope_bindings: &HashSet<Atom>) -> (Vec<usize>, Vec<usize>) {
        let shadowed = self.shadowing_indices(scope_bindings);
        let captured = self.capture_indices(scope_bindings);
        self.enter_shadowed(&shadowed);
        self.enter_captured(&captured);
        (shadowed, captured)
    }

    fn exit_scope(&mut self, shadowed: &[usize], captured: &[usize]) {
        self.exit_captured(captured);
        self.exit_shadowed(shadowed);
    }
}

impl VisitMut for ComputedLocalInliner {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Ident(ident) = expr {
            if let Some(index) = self.active_index(ident.sym.as_ref()) {
                *expr = self.bindings[index].1.clone();
                expr.visit_mut_children_with(self);
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut swc_core::ecma::ast::ArrowExpr) {
        let scope_bindings = arrow_scope_bindings(arrow);
        let (shadowed, captured) = self.enter_scope(&scope_bindings);
        arrow.body.visit_mut_with(self);
        self.exit_scope(&shadowed, &captured);
    }

    fn visit_mut_function(&mut self, function: &mut swc_core::ecma::ast::Function) {
        let scope_bindings = function_scope_bindings(function);
        let (shadowed, captured) = self.enter_scope(&scope_bindings);
        if let Some(body) = function.body.as_mut() {
            body.visit_mut_with(self);
        }
        self.exit_scope(&shadowed, &captured);
    }
}

fn arrow_scope_bindings(arrow: &swc_core::ecma::ast::ArrowExpr) -> HashSet<Atom> {
    let mut bindings = HashSet::new();
    for param in &arrow.params {
        collect_pat_bindings(param, &mut bindings);
    }
    collect_block_or_expr_scope_bindings(arrow.body.as_ref(), &mut bindings);
    bindings
}

fn function_scope_bindings(function: &swc_core::ecma::ast::Function) -> HashSet<Atom> {
    let mut bindings = HashSet::new();
    for param in &function.params {
        collect_pat_bindings(&param.pat, &mut bindings);
    }
    if let Some(body) = function.body.as_ref() {
        collect_stmt_scope_bindings(&body.stmts, &mut bindings);
    }
    bindings
}

fn collect_block_or_expr_scope_bindings(body: &BlockStmtOrExpr, bindings: &mut HashSet<Atom>) {
    if let BlockStmtOrExpr::BlockStmt(block) = body {
        collect_stmt_scope_bindings(&block.stmts, bindings);
    }
}

fn collect_stmt_scope_bindings(stmts: &[Stmt], bindings: &mut HashSet<Atom>) {
    for stmt in stmts {
        match stmt {
            Stmt::Decl(Decl::Var(var)) => {
                for decl in &var.decls {
                    collect_pat_bindings(&decl.name, bindings);
                }
            }
            Stmt::Decl(Decl::Fn(function)) => {
                bindings.insert(function.ident.sym.clone());
            }
            Stmt::Decl(Decl::Class(class)) => {
                bindings.insert(class.ident.sym.clone());
            }
            Stmt::Block(block) => collect_stmt_scope_bindings(&block.stmts, bindings),
            Stmt::If(if_stmt) => {
                collect_stmt_scope_binding(if_stmt.cons.as_ref(), bindings);
                if let Some(alt) = if_stmt.alt.as_ref() {
                    collect_stmt_scope_binding(alt.as_ref(), bindings);
                }
            }
            _ => {}
        }
    }
}

fn collect_stmt_scope_binding(stmt: &Stmt, bindings: &mut HashSet<Atom>) {
    match stmt {
        Stmt::Block(block) => collect_stmt_scope_bindings(&block.stmts, bindings),
        stmt => collect_stmt_scope_bindings(std::slice::from_ref(stmt), bindings),
    }
}

fn computed_if_return_chain_expr(
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> Result<Option<String>> {
    let mut branches = Vec::new();
    let mut aliases = HashMap::new();

    for stmt in stmts {
        match stmt {
            Stmt::Decl(Decl::Var(var))
                if branches.is_empty()
                    && collect_computed_setup_prop_alias_var(var, ctx, &mut aliases) =>
            {
                continue;
            }
            Stmt::If(if_stmt) => {
                let Some(expr) = direct_return_expr_from_stmt(if_stmt.cons.as_ref()) else {
                    return Ok(None);
                };
                if if_stmt.alt.is_some() {
                    return Ok(None);
                }
                let test = inline_computed_alias_expr(if_stmt.test.as_ref(), &aliases);
                let expr = inline_computed_alias_expr(expr, &aliases);
                branches.push((
                    clean_expr(&print_expr(&test, ctx)?, ctx),
                    clean_expr(&print_expr(&expr, ctx)?, ctx),
                ));
            }
            Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            }) if !branches.is_empty() => {
                let expr = inline_computed_alias_expr(expr, &aliases);
                let fallback = clean_expr(&print_expr(&expr, ctx)?, ctx);
                return Ok(Some(format_conditional_expr(&branches, fallback)));
            }
            _ => return Ok(None),
        }
    }

    Ok(None)
}

fn inline_computed_alias_expr(expr: &Expr, aliases: &HashMap<Atom, Expr>) -> Expr {
    if aliases.is_empty() {
        return expr.clone();
    }

    let mut expr = expr.clone();
    expr.visit_mut_with(&mut ComputedLocalInliner::new(aliases.clone()));
    expr
}

pub(super) fn return_expr_from_stmt(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Return(ReturnStmt {
            arg: Some(expr), ..
        }) => Some(expr.as_ref()),
        Stmt::Block(block) => block.stmts.iter().find_map(return_expr_from_stmt),
        _ => None,
    }
}

fn direct_return_expr_from_stmt(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Return(ReturnStmt {
            arg: Some(expr), ..
        }) => Some(expr.as_ref()),
        Stmt::Block(block) => {
            let [Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            })] = block.stmts.as_slice()
            else {
                return None;
            };
            Some(expr.as_ref())
        }
        _ => None,
    }
}

fn format_conditional_expr(branches: &[(String, String)], fallback: String) -> String {
    branches
        .iter()
        .rev()
        .fold(fallback, |alternate, (condition, consequent)| {
            format!("{condition} ? {consequent} : {alternate}")
        })
}

pub(super) fn resolve_component_name(expr: &Expr, ctx: &VueRecoveryContext) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if helper_name(&call.callee, ctx) != Some(VueHelper::ResolveComponent) {
        return None;
    }
    call.args
        .first()
        .and_then(|arg| string_lit(arg.expr.as_ref()))
}

pub(in crate::vue_recovery) fn resolve_directive_name(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if helper_name(&call.callee, ctx) != Some(VueHelper::ResolveDirective) {
        return None;
    }
    call.args
        .first()
        .and_then(|arg| string_lit(arg.expr.as_ref()))
}
