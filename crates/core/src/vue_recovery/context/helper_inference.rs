use super::*;

pub(in crate::vue_recovery) fn infer_render_helpers(
    render: RenderSource<'_>,
    ctx: &mut VueRecoveryContext,
) {
    if ctx.vue_helper_candidates.is_empty() && ctx.vue_helpers.is_empty() {
        return;
    }

    let mut inference = HelperInference {
        candidates: &ctx.vue_helper_candidates,
        known_helpers: &ctx.vue_helpers,
        import_ctxts: &ctx.top_level_binding_ctxts,
        unresolved_ctxt: ctx.unresolved_ctxt,
        inferred: HashMap::new(),
        prop_value_depth: 0,
        vnode_child_depth: 0,
    };
    match render {
        RenderSource::Function { render, .. } => {
            if let Some(body) = render.function.body.as_ref() {
                body.visit_with(&mut inference);
            }
        }
        RenderSource::SetupArrow { render, .. } => render.body.visit_with(&mut inference),
    }

    for (local, helper) in inference.inferred {
        ctx.vue_helpers.entry(local).or_insert(helper);
    }
}

struct HelperInference<'a> {
    candidates: &'a std::collections::HashSet<Atom>,
    known_helpers: &'a HashMap<Atom, VueHelper>,
    import_ctxts: &'a HashMap<Atom, SyntaxContext>,
    unresolved_ctxt: SyntaxContext,
    inferred: HashMap<Atom, VueHelper>,
    prop_value_depth: usize,
    vnode_child_depth: usize,
}

impl HelperInference<'_> {
    /// True if `ident` refers to the imported binding of its name (matching the
    /// import's resolved `SyntaxContext`), not an inner-scope local reusing the
    /// name. Every helper candidate/known helper is an import, so recognition
    /// requires this rather than falling back to name-only matching.
    fn import_resolves(&self, ident: &Ident) -> bool {
        self.import_ctxts
            .get(&ident.sym)
            .is_some_and(|ctxt| *ctxt == ident.ctxt)
    }

    /// A helper candidate reference that resolves to its import binding.
    fn is_candidate(&self, ident: &Ident) -> bool {
        self.candidates.contains(&ident.sym) && self.import_resolves(ident)
    }
}

impl Visit for HelperInference<'_> {
    fn visit_if_stmt(&mut self, if_stmt: &IfStmt) {
        self.infer_condition_unref_expr(if_stmt.test.as_ref());
        if_stmt.visit_children_with(self);
    }

    fn visit_cond_expr(&mut self, cond: &CondExpr) {
        self.infer_condition_unref_expr(cond.test.as_ref());
        cond.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        self.infer_unref_expr(member.obj.as_ref());
        member.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if self.prop_value_depth > 0 {
            self.infer_render_prop_unref_call(call);
        }
        if self.vnode_child_depth > 0 {
            self.infer_vnode_child_helper_call(call);
        }

        if let Callee::Expr(callee) = &call.callee {
            self.infer_unref_expr(callee.as_ref());
        }

        if let Some((callee, inferred_fragment)) = self.fragment_block_call(call) {
            self.inferred
                .insert(callee.sym.clone(), VueHelper::CreateElementBlock);
            if let Some(fragment) = inferred_fragment {
                self.inferred
                    .insert(fragment.sym.clone(), VueHelper::Fragment);
            }
        }

        if let Some(callee) = self.with_directives_call(call) {
            self.inferred
                .entry(callee.sym.clone())
                .or_insert(VueHelper::WithDirectives);
        }

        if let Some(callee) = call_callee_ident(call) {
            if self.is_candidate(callee) {
                if let Some(helper) = infer_call_helper(call) {
                    self.inferred.entry(callee.sym.clone()).or_insert(helper);
                }
            }
        }

        if let Some(VueHelper::CreateElementBlock | VueHelper::CreateElementVNode) =
            self.call_helper(call)
        {
            if let Some(fragment) = call
                .args
                .first()
                .and_then(|arg| ident_expr(arg.expr.as_ref()))
                .filter(|&ident| self.is_candidate(ident))
            {
                self.inferred
                    .entry(fragment.sym.clone())
                    .or_insert(VueHelper::Fragment);
            }
        }

        if matches!(
            self.call_helper(call),
            Some(VueHelper::CreateBlock | VueHelper::CreateVNode)
        ) {
            self.infer_builtin_component_arg(call);
        }

        if matches!(self.call_helper(call), Some(VueHelper::RenderList)) {
            if let Some(source) = call.args.first() {
                self.infer_render_list_source_unref(source.expr.as_ref());
            }
        }

        if matches!(
            self.call_helper(call),
            Some(
                VueHelper::CreateBlock
                    | VueHelper::CreateElementBlock
                    | VueHelper::CreateElementVNode
                    | VueHelper::CreateVNode
            )
        ) {
            self.infer_render_prop_unrefs(call);
            self.infer_render_child_helpers(call);
        }

        call.visit_children_with(self);
    }
}

impl HelperInference<'_> {
    fn call_helper(&self, call: &CallExpr) -> Option<VueHelper> {
        call_callee_ident(call).and_then(|callee| {
            self.inferred
                .get(&callee.sym)
                .or_else(|| {
                    self.import_resolves(callee)
                        .then(|| self.known_helpers.get(&callee.sym))
                        .flatten()
                })
                .cloned()
        })
    }

    fn fragment_block_call<'a>(
        &self,
        call: &'a CallExpr,
    ) -> Option<(
        &'a swc_core::ecma::ast::Ident,
        Option<&'a swc_core::ecma::ast::Ident>,
    )> {
        let callee = call_callee_ident(call)?;
        if !self.is_candidate(callee) {
            return None;
        }
        if !is_fragment_patch_flag(call.args.get(3).map(|arg| arg.expr.as_ref())) {
            return None;
        }
        let fragment = call
            .args
            .first()
            .and_then(|arg| ident_expr(arg.expr.as_ref()))?;
        if self.is_candidate(fragment) || self.import_resolves(fragment) {
            return Some((callee, Some(fragment)));
        }
        if self.is_unresolved_fragment_name(fragment) {
            return Some((callee, None));
        }
        None
    }

    fn is_unresolved_fragment_name(&self, ident: &Ident) -> bool {
        ident.sym.as_ref() == "Fragment" && ident.ctxt == self.unresolved_ctxt
    }

    fn with_directives_call<'a>(
        &self,
        call: &'a CallExpr,
    ) -> Option<&'a swc_core::ecma::ast::Ident> {
        let callee = call_callee_ident(call)?;
        if !is_with_directives_call(&call.args) {
            return None;
        }
        let base = call.args.first()?;
        self.is_likely_vnode_expr(base.expr.as_ref())
            .then_some(callee)
    }

    fn is_likely_vnode_expr(&self, expr: &Expr) -> bool {
        match unwrap_paren_expr(expr) {
            Expr::Seq(seq) => seq
                .exprs
                .last()
                .is_some_and(|expr| self.is_likely_vnode_expr(expr.as_ref())),
            Expr::Call(call) => self
                .call_helper(call)
                .or_else(|| infer_call_helper(call))
                .is_some_and(|helper| {
                    matches!(
                        helper,
                        VueHelper::CreateBlock
                            | VueHelper::CreateElementBlock
                            | VueHelper::CreateElementVNode
                            | VueHelper::CreateVNode
                    )
                }),
            _ => false,
        }
    }

    fn infer_unref_expr(&mut self, expr: &Expr) {
        let Expr::Call(call) = unwrap_paren_expr(expr) else {
            return;
        };
        if !is_display_string_call(&call.args) {
            return;
        }
        let Some(callee) = call_callee_ident(call) else {
            return;
        };
        if !self.is_candidate(callee) {
            return;
        }
        self.inferred.insert(callee.sym.clone(), VueHelper::Unref);
    }

    fn infer_condition_unref_expr(&mut self, expr: &Expr) {
        match unwrap_paren_expr(expr) {
            Expr::Call(_) => self.infer_unref_expr(expr),
            Expr::Unary(unary) if unary.op == UnaryOp::Bang => {
                self.infer_condition_unref_expr(unary.arg.as_ref());
            }
            Expr::Bin(bin)
                if matches!(
                    bin.op,
                    BinaryOp::LogicalAnd
                        | BinaryOp::LogicalOr
                        | BinaryOp::EqEq
                        | BinaryOp::EqEqEq
                        | BinaryOp::NotEq
                        | BinaryOp::NotEqEq
                ) =>
            {
                self.infer_condition_unref_expr(bin.left.as_ref());
                self.infer_condition_unref_expr(bin.right.as_ref());
            }
            Expr::Cond(cond) => {
                self.infer_condition_unref_expr(cond.test.as_ref());
            }
            _ => {}
        }
    }

    fn infer_render_prop_unrefs(&mut self, call: &CallExpr) {
        let Some(props) = call.args.get(1).and_then(|arg| match arg.expr.as_ref() {
            Expr::Object(object) => Some(object),
            _ => None,
        }) else {
            return;
        };

        self.prop_value_depth += 1;
        for prop in &props.props {
            match prop {
                PropOrSpread::Prop(prop) => {
                    if let Prop::KeyValue(key_value) = prop.as_ref() {
                        key_value.value.visit_with(self);
                    }
                }
                PropOrSpread::Spread(spread) => {
                    spread.expr.visit_with(self);
                }
            }
        }
        self.prop_value_depth -= 1;
    }

    fn infer_render_prop_unref_call(&mut self, call: &CallExpr) {
        if !is_render_prop_unref_call(&call.args) {
            return;
        }
        let Some(callee) = call_callee_ident(call) else {
            return;
        };
        if !self.is_candidate(callee) {
            return;
        }
        self.inferred.insert(callee.sym.clone(), VueHelper::Unref);
    }

    fn infer_render_child_helpers(&mut self, call: &CallExpr) {
        let Some(children) = call.args.get(2) else {
            return;
        };
        self.vnode_child_depth += 1;
        children.expr.visit_with(self);
        self.vnode_child_depth -= 1;
    }

    fn infer_vnode_child_helper_call(&mut self, call: &CallExpr) {
        if !is_static_text_vnode_call(&call.args) {
            return;
        }
        let Some(callee) = call_callee_ident(call) else {
            return;
        };
        if !self.is_candidate(callee) {
            return;
        }
        self.inferred
            .entry(callee.sym.clone())
            .or_insert(VueHelper::CreateTextVNode);
    }

    fn infer_render_list_source_unref(&mut self, expr: &Expr) {
        let Expr::Call(call) = unwrap_paren_expr(expr) else {
            return;
        };
        self.infer_render_prop_unref_call(call);
    }

    fn infer_builtin_component_arg(&mut self, call: &CallExpr) {
        let Some(component) = call
            .args
            .first()
            .and_then(|arg| ident_expr(arg.expr.as_ref()))
            .filter(|&ident| self.is_candidate(ident))
        else {
            return;
        };
        let Some(props) = call.args.get(1).and_then(|arg| match arg.expr.as_ref() {
            Expr::Object(object) => Some(object),
            _ => None,
        }) else {
            return;
        };
        if is_transition_component_props(props) {
            self.inferred
                .entry(component.sym.clone())
                .or_insert(VueHelper::Other("Transition".to_string()));
        }
    }
}

fn is_render_prop_unref_call(args: &[ExprOrSpread]) -> bool {
    if args.len() != 1 {
        return false;
    }
    matches!(
        args.first().map(|arg| unwrap_paren_expr(arg.expr.as_ref())),
        Some(Expr::Ident(_) | Expr::Member(_) | Expr::OptChain(_))
    )
}

fn is_transition_component_props(object: &ObjectLit) -> bool {
    object.props.iter().any(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return false;
        };
        matches!(
            prop_name(&key_value.key).as_deref(),
            Some(
                "onBeforeEnter"
                    | "onEnter"
                    | "onAfterEnter"
                    | "onEnterCancelled"
                    | "onBeforeLeave"
                    | "onLeave"
                    | "onAfterLeave"
                    | "onLeaveCancelled"
            )
        )
    })
}

fn is_fragment_patch_flag(expr: Option<&Expr>) -> bool {
    matches!(
        expr,
        Some(Expr::Lit(Lit::Num(number)))
            if matches!(number.value as i32, 64 | 128 | 256)
    )
}

pub(in crate::vue_recovery) fn unwrap_paren_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => unwrap_paren_expr(paren.expr.as_ref()),
        _ => expr,
    }
}

fn infer_call_helper(call: &CallExpr) -> Option<VueHelper> {
    if is_with_directives_call(&call.args) {
        return Some(VueHelper::WithDirectives);
    }
    if is_with_memo_call(&call.args) {
        return Some(VueHelper::WithMemo);
    }
    if is_create_slots_call(&call.args) {
        return Some(VueHelper::CreateSlots);
    }
    if is_render_slot_call(&call.args) {
        return Some(VueHelper::RenderSlot);
    }
    if is_render_list_call(&call.args) {
        return Some(VueHelper::RenderList);
    }
    if is_event_modifier_helper_call(&call.args) {
        return Some(VueHelper::WithModifiers);
    }
    if is_with_ctx_call(&call.args) {
        return Some(VueHelper::WithCtx);
    }
    if is_create_static_vnode_call(&call.args) {
        return Some(VueHelper::CreateStaticVNode);
    }
    if is_create_comment_vnode_call(&call.args) {
        return Some(VueHelper::CreateCommentVNode);
    }
    if is_create_text_vnode_call(&call.args) {
        return Some(VueHelper::CreateTextVNode);
    }
    if is_element_vnode_call(&call.args) {
        return Some(VueHelper::CreateElementBlock);
    }
    if is_component_vnode_call(&call.args) {
        return Some(VueHelper::CreateVNode);
    }
    if is_resolve_component_call(&call.args) {
        return Some(VueHelper::ResolveComponent);
    }
    if is_display_string_call(&call.args) {
        return Some(VueHelper::ToDisplayString);
    }
    if is_open_block_call(&call.args) {
        return Some(VueHelper::OpenBlock);
    }
    None
}

fn is_with_directives_call(args: &[ExprOrSpread]) -> bool {
    matches!(args.get(1).map(|arg| arg.expr.as_ref()), Some(Expr::Array(array)) if array.elems.iter().flatten().any(|elem| matches!(elem.expr.as_ref(), Expr::Array(_))))
}

fn is_with_memo_call(args: &[ExprOrSpread]) -> bool {
    args.len() >= 4
        && matches!(
            args.get(1).map(|arg| arg.expr.as_ref()),
            Some(Expr::Arrow(_))
        )
}

fn is_create_slots_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.first().map(|arg| arg.expr.as_ref()),
        Some(Expr::Object(_))
    ) && matches!(
        args.get(1).map(|arg| arg.expr.as_ref()),
        Some(Expr::Array(_))
    )
}

fn is_render_slot_call(args: &[ExprOrSpread]) -> bool {
    args.len() >= 2
        && args
            .first()
            .is_some_and(|arg| is_slots_source_expr(arg.expr.as_ref()))
}

fn is_slots_source_expr(expr: &Expr) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Ident(ident) => matches!(ident.sym.as_ref(), "$slots" | "slots"),
        Expr::Member(member) => is_slots_member_prop(&member.prop),
        _ => false,
    }
}

pub(super) fn is_slots_member_prop(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(ident) => ident.sym.as_ref() == "$slots",
        MemberProp::Computed(computed) => {
            string_lit(computed.expr.as_ref()).as_deref() == Some("$slots")
        }
        MemberProp::PrivateName(_) => false,
    }
}

pub(super) fn is_setup_slots_member_prop(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(ident) => matches!(ident.sym.as_ref(), "$slots" | "slots"),
        MemberProp::Computed(computed) => {
            matches!(
                string_lit(computed.expr.as_ref()).as_deref(),
                Some("$slots" | "slots")
            )
        }
        MemberProp::PrivateName(_) => false,
    }
}

fn is_render_list_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.get(1).map(|arg| arg.expr.as_ref()),
        Some(Expr::Arrow(_))
    )
}

fn is_event_modifier_helper_call(args: &[ExprOrSpread]) -> bool {
    if args.len() != 2 {
        return false;
    }

    let Some(modifiers) = args.get(1).and_then(|arg| match arg.expr.as_ref() {
        Expr::Array(array) => Some(array),
        _ => None,
    }) else {
        return false;
    };
    if modifiers
        .elems
        .iter()
        .flatten()
        .any(|elem| string_lit(elem.expr.as_ref()).is_none())
    {
        return false;
    }

    matches!(
        args.first().map(|arg| unwrap_paren_expr(arg.expr.as_ref())),
        Some(
            Expr::Ident(_)
                | Expr::Member(_)
                | Expr::Call(_)
                | Expr::Arrow(_)
                | Expr::Fn(_)
                | Expr::Bin(_)
                | Expr::Assign(_)
        )
    )
}

fn is_with_ctx_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.first().map(|arg| arg.expr.as_ref()),
        Some(Expr::Arrow(_))
    )
}

fn is_create_static_vnode_call(args: &[ExprOrSpread]) -> bool {
    args.first()
        .and_then(|arg| string_lit(arg.expr.as_ref()))
        .is_some_and(|value| value.contains('<'))
}

fn is_create_comment_vnode_call(args: &[ExprOrSpread]) -> bool {
    args.first()
        .is_some_and(|arg| string_lit(arg.expr.as_ref()).is_some())
        && matches!(
            args.get(1).map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Bool(_)))
        )
}

fn is_create_text_vnode_call(args: &[ExprOrSpread]) -> bool {
    args.get(1)
        .is_some_and(|arg| is_numeric_expr(arg.expr.as_ref()))
}

fn is_static_text_vnode_call(args: &[ExprOrSpread]) -> bool {
    matches!(args.len(), 1 | 2)
        && args
            .first()
            .is_some_and(|arg| string_lit(arg.expr.as_ref()).is_some())
        && args
            .get(1)
            .is_none_or(|arg| is_numeric_expr(arg.expr.as_ref()))
}

fn is_numeric_expr(expr: &Expr) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Lit(Lit::Num(_)) => true,
        Expr::Unary(unary) if unary.op == UnaryOp::Minus => {
            matches!(
                unwrap_paren_expr(unary.arg.as_ref()),
                Expr::Lit(Lit::Num(_))
            )
        }
        _ => false,
    }
}

fn is_element_vnode_call(args: &[ExprOrSpread]) -> bool {
    args.len() >= 2
        && args
            .first()
            .and_then(|arg| string_lit(arg.expr.as_ref()))
            .is_some_and(|value| !value.contains('<'))
}

fn is_component_vnode_call(args: &[ExprOrSpread]) -> bool {
    args.len() >= 2
        && !matches!(
            args.first().map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Str(_)) | Expr::Object(_))
        )
}

fn is_resolve_component_call(args: &[ExprOrSpread]) -> bool {
    args.len() == 1
        && args
            .first()
            .is_some_and(|arg| string_lit(arg.expr.as_ref()).is_some())
}

fn is_display_string_call(args: &[ExprOrSpread]) -> bool {
    args.len() == 1
        && args
            .first()
            .is_none_or(|arg| string_lit(arg.expr.as_ref()).is_none())
}

fn is_open_block_call(args: &[ExprOrSpread]) -> bool {
    args.is_empty()
        || matches!(
            args.first().map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Bool(_)))
        )
}

pub(in crate::vue_recovery) fn call_callee_ident(
    call: &CallExpr,
) -> Option<&swc_core::ecma::ast::Ident> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    ident_expr(callee.as_ref())
}

pub(super) fn ident_expr(expr: &Expr) -> Option<&swc_core::ecma::ast::Ident> {
    match expr {
        Expr::Ident(ident) => Some(ident),
        _ => None,
    }
}
