use crate::collections::{HashMap, HashSet};

use swc_core::atoms::{Atom, Wtf8Atom};
use swc_core::common::util::take::Take;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, AssignExpr, AssignOp, BindingIdent, BlockStmt, Bool, CallExpr,
    Callee, Class, Decl, Expr, ExprOrSpread, Function, FunctionBody, Ident, ImportDecl,
    ImportSpecifier, JSXAttr, JSXAttrName, JSXAttrOrSpread, JSXAttrValue, JSXClosingElement,
    JSXClosingFragment, JSXElement, JSXElementChild, JSXElementName, JSXExpr, JSXExprContainer,
    JSXFragment, JSXMemberExpr, JSXNamespacedName, JSXObject, JSXOpeningElement,
    JSXOpeningFragment, JSXSpreadChild, JSXText, KeyValueProp, Lit, MemberExpr, MemberProp, Module,
    ModuleDecl, ModuleExportName, ModuleItem, NewExpr, Number, ObjectLit, OptCall, Param, Pat,
    Prop, PropName, PropOrSpread, ReturnStmt, SpreadElement, Stmt, Str, TaggedTpl, VarDecl,
    VarDeclKind, VarDeclarator,
};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;
use crate::js_names::to_valid_identifier_name;

use super::decl_utils::{fresh_binding_ident, BindingId};
use super::rename_utils::{
    collect_exported_binding_ids_from_items, rename_bindings, starts_with_lowercase, BindingRename,
};
use super::RewriteLevel;

const CLASSIC_PRAGMA: &str = "createElement";

fn is_automatic_pragma(name: &str) -> bool {
    matches!(
        name,
        "jsx" | "jsxs" | "_jsx" | "_jsxs" | "jsxDEV" | "jsxsDEV"
    )
}

fn is_jsx_pragma_name(name: &str) -> bool {
    matches!(
        name,
        "createElement" | "jsx" | "jsxs" | "_jsx" | "_jsxs" | "jsxDEV" | "jsxsDEV"
    )
}

pub struct UnJsx {
    unresolved_mark: Mark,
    level: RewriteLevel,
    pending_stmts: Vec<Vec<Stmt>>,
    /// The function depth each `pending_stmts` frame was opened at. An alias
    /// may only be hoisted into a frame of the current depth: a frame of an
    /// enclosing function runs in a different scope and at a different time.
    frame_depths: Vec<usize>,
    function_depth: usize,
    used_names: Vec<HashSet<Atom>>,
    string_consts: Vec<HashMap<BindingId, Str>>,
    import_pragmas: HashMap<BindingId, &'static str>,
    converted_classic_pragma: bool,
}

impl UnJsx {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self::new_with_level(unresolved_mark, RewriteLevel::Standard)
    }

    pub fn new_with_level(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
            pending_stmts: Vec::new(),
            frame_depths: Vec::new(),
            function_depth: 0,
            used_names: Vec::new(),
            string_consts: Vec::new(),
            import_pragmas: HashMap::default(),
            converted_classic_pragma: false,
        }
    }

    fn should_run(module: &Module) -> bool {
        struct Scan {
            found: bool,
        }
        impl Visit for Scan {
            fn visit_import_decl(&mut self, import: &ImportDecl) {
                if self.found {
                    return;
                }
                if let Some(src) = import.src.value.as_str() {
                    if matches!(src, "react/jsx-runtime" | "react/jsx-dev-runtime") {
                        self.found = true;
                    }
                }
            }
            fn visit_call_expr(&mut self, call: &CallExpr) {
                if self.found {
                    return;
                }
                if let Callee::Expr(expr) = &call.callee {
                    let is_pragma = match expr.as_ref() {
                        Expr::Ident(id) => is_jsx_pragma_name(id.sym.as_ref()),
                        Expr::Member(m) => {
                            if let MemberProp::Ident(prop) = &m.prop {
                                is_jsx_pragma_name(prop.sym.as_ref())
                                    && !(prop.sym.as_ref() == CLASSIC_PRAGMA
                                        && matches!(m.obj.as_ref(), Expr::Ident(obj) if obj.sym.as_ref() == "document"))
                            } else {
                                false
                            }
                        }
                        _ => false,
                    };
                    if is_pragma {
                        self.found = true;
                        return;
                    }
                }
                call.visit_children_with(self);
            }
            fn visit_assign_expr(&mut self, assign: &AssignExpr) {
                if self.found {
                    return;
                }
                if let swc_core::ecma::ast::AssignTarget::Simple(
                    swc_core::ecma::ast::SimpleAssignTarget::Member(member),
                ) = &assign.left
                {
                    if let MemberProp::Ident(prop) = &member.prop {
                        if prop.sym.as_ref() == "displayName" {
                            self.found = true;
                            return;
                        }
                    }
                }
                assign.visit_children_with(self);
            }
        }
        let mut scan = Scan { found: false };
        module.visit_with(&mut scan);
        scan.found
    }

    fn process_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        self.converted_classic_pragma = false;
        self.import_pragmas = collect_import_pragmas(items);
        let (renames, name_registry) =
            collect_module_renames(items, self.unresolved_mark, &self.import_pragmas);
        rename_bindings(items, &renames);

        self.used_names.push(name_registry);
        self.string_consts
            .push(collect_string_consts_from_module_items(items));

        let old = std::mem::take(items);
        let mut rewritten = Vec::with_capacity(old.len());
        for mut item in old {
            self.push_pending_frame();
            item.visit_mut_with(self);
            let pending = self.pop_pending_frame();
            rewritten.extend(pending.into_iter().map(ModuleItem::Stmt));
            rewritten.push(item);
        }

        self.string_consts.pop();
        self.used_names.pop();
        *items = rewritten;

        if self.converted_classic_pragma {
            strip_unused_classic_pragma_imports(items);
        }
    }

    fn process_stmts(&mut self, stmts: &mut Vec<Stmt>, list_is_function_scope: bool) {
        let (renames, name_registry) = collect_stmt_renames(
            stmts,
            self.unresolved_mark,
            &self.import_pragmas,
            list_is_function_scope,
        );
        rename_bindings(stmts, &renames);

        self.used_names.push(name_registry);
        self.string_consts
            .push(collect_string_consts_from_stmts(stmts));

        let old = std::mem::take(stmts);
        let mut rewritten = Vec::with_capacity(old.len());
        for mut stmt in old {
            self.push_pending_frame();
            stmt.visit_mut_with(self);
            let pending = self.pop_pending_frame();
            rewritten.extend(pending);
            rewritten.push(stmt);
        }

        self.string_consts.pop();
        self.used_names.pop();
        *stmts = rewritten;
    }

    fn convert_call(&mut self, call: &CallExpr) -> Option<Expr> {
        let pragma = get_pragma(&call.callee, &self.import_pragmas)?;
        if call.args.len() < 2 {
            return None;
        }

        let type_arg = &call.args[0];
        if type_arg.spread.is_some() {
            return None;
        }

        let type_expr = type_arg.expr.as_ref();
        if is_capitalization_invalid(type_expr) {
            return None;
        }

        let mut tag = self.to_jsx_element_name(type_expr);
        if let Some(inlined) = self.inline_const_string_tag(type_expr) {
            tag = self
                .to_jsx_element_name(&Expr::Lit(Lit::Str(inlined)))
                .or(tag);
        }

        if tag.is_none() {
            let should_alias = self.level >= RewriteLevel::Aggressive
                || (self.level >= RewriteLevel::Standard
                    && self.has_strong_jsx_shape(pragma, call));
            if should_alias {
                let base = if expr_contains_inlined_jsx_component(type_expr) {
                    "InlineComponent"
                } else {
                    "Component"
                };
                tag = self
                    .create_component_alias(type_expr, base)
                    .map(JSXElementName::Ident);
            }
        }

        let tag = tag?;
        let mut attrs = self.to_jsx_attrs(&call.args[1])?;
        let automatic = is_automatic_pragma(pragma);

        let mut children = if automatic {
            let extracted = extract_children_attr(&mut attrs)
                .into_iter()
                .flat_map(|value| self.jsx_attr_value_to_children(value))
                .collect::<Vec<_>>();

            if call.args.len() >= 3 {
                let key_arg = &call.args[2];
                if key_arg.spread.is_none() && !is_undefined_expr(&key_arg.expr) {
                    attrs.insert(
                        0,
                        JSXAttrOrSpread::JSXAttr(JSXAttr {
                            span: DUMMY_SP,
                            name: JSXAttrName::Ident("key".into()),
                            value: Some(self.expr_to_attr_value(key_arg.expr.as_ref())),
                        }),
                    );
                }
            }
            extracted
        } else {
            call.args[2..]
                .iter()
                .filter_map(|arg| self.expr_or_spread_to_child(arg))
                .collect::<Vec<_>>()
        };

        if is_fragment_name(&tag) && attrs.is_empty() {
            if pragma == CLASSIC_PRAGMA {
                self.converted_classic_pragma = true;
            }
            let outer_span = if call.span.lo.0 != 0 {
                call.span
            } else {
                DUMMY_SP
            };
            return Some(Expr::JSXFragment(JSXFragment {
                span: outer_span,
                opening: JSXOpeningFragment { span: DUMMY_SP },
                children,
                closing: JSXClosingFragment { span: DUMMY_SP },
            }));
        }

        let self_closing = children.is_empty();
        let closing = (!self_closing).then_some(JSXClosingElement {
            span: DUMMY_SP,
            name: tag.clone(),
        });

        if pragma == CLASSIC_PRAGMA {
            self.converted_classic_pragma = true;
        }
        let outer_span = if call.span.lo.0 != 0 {
            call.span
        } else {
            DUMMY_SP
        };
        Some(Expr::JSXElement(Box::new(JSXElement {
            span: outer_span,
            opening: JSXOpeningElement {
                name: tag,
                span: DUMMY_SP,
                attrs,
                self_closing,
                type_args: None,
            },
            children: std::mem::take(&mut children),
            closing,
        })))
    }

    fn inline_const_string_tag(&self, expr: &Expr) -> Option<Str> {
        let Expr::Ident(ident) = expr else {
            return None;
        };
        let id = (ident.sym.clone(), ident.ctxt);
        for scope in self.string_consts.iter().rev() {
            if let Some(value) = scope.get(&id) {
                return Some(value.clone());
            }
        }
        None
    }

    fn push_pending_frame(&mut self) {
        self.pending_stmts.push(Vec::new());
        self.frame_depths.push(self.function_depth);
    }

    fn pop_pending_frame(&mut self) -> Vec<Stmt> {
        self.frame_depths.pop();
        self.pending_stmts.pop().unwrap()
    }

    /// `None` when no statement list of the current function can take the
    /// alias declaration: the expression sits in a parameter default, a class
    /// field initializer, or another position outside any statement list of
    /// its own function.
    fn create_component_alias(&mut self, expr: &Expr, base: &str) -> Option<Ident> {
        if self.frame_depths.last() != Some(&self.function_depth) {
            return None;
        }
        let name = self.generate_name(base.to_string());
        let ident = fresh_binding_ident(name.clone().into(), DUMMY_SP);
        if let Some(pending) = self.pending_stmts.last_mut() {
            pending.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                span: DUMMY_SP,
                ctxt: SyntaxContext::empty(),
                kind: VarDeclKind::Const,
                declare: false,
                decls: vec![VarDeclarator {
                    span: DUMMY_SP,
                    name: Pat::Ident(BindingIdent {
                        id: ident.clone(),
                        type_ann: None,
                    }),
                    init: Some(Box::new(expr.clone())),
                    definite: false,
                }],
            }))));
        }
        Some(ident)
    }

    fn generate_name(&mut self, base: String) -> String {
        let names = self.used_names.last_mut().expect("body scope should exist");
        let base_atom = Atom::from(base.as_str());
        if !names.contains(&base_atom) {
            names.insert(base_atom);
            return base;
        }

        let mut idx = 1usize;
        loop {
            let candidate = format!("{base}_{idx}");
            let candidate_atom = Atom::from(candidate.as_str());
            if !names.contains(&candidate_atom) {
                names.insert(candidate_atom);
                return candidate;
            }
            idx += 1;
        }
    }

    fn to_jsx_element_name(&self, expr: &Expr) -> Option<JSXElementName> {
        match expr {
            Expr::Lit(Lit::Str(s)) => jsx_name_from_string(s, self.unresolved_mark),
            Expr::Tpl(_) => {
                let value = no_substitution_template_str(expr)?;
                jsx_name_from_string(&value, self.unresolved_mark)
            }
            Expr::Ident(ident) => Some(JSXElementName::Ident(ident.clone())),
            Expr::Member(member) => self.member_expr_to_jsx_name(member),
            _ => None,
        }
    }

    fn member_expr_to_jsx_name(&self, member: &MemberExpr) -> Option<JSXElementName> {
        let prop = match &member.prop {
            MemberProp::Ident(ident) => ident.clone(),
            _ => return None,
        };

        let obj = self.expr_to_jsx_object(&member.obj)?;
        Some(JSXElementName::JSXMemberExpr(JSXMemberExpr {
            span: DUMMY_SP,
            obj,
            prop,
        }))
    }

    fn expr_to_jsx_object(&self, expr: &Expr) -> Option<JSXObject> {
        match expr {
            Expr::Ident(ident) => Some(JSXObject::Ident(ident.clone())),
            Expr::Member(member) => {
                let prop = match &member.prop {
                    MemberProp::Ident(ident) => ident.clone(),
                    _ => return None,
                };
                let obj = self.expr_to_jsx_object(&member.obj)?;
                Some(JSXObject::JSXMemberExpr(Box::new(JSXMemberExpr {
                    span: DUMMY_SP,
                    obj,
                    prop,
                })))
            }
            _ => None,
        }
    }

    fn to_jsx_attrs(&self, props_arg: &ExprOrSpread) -> Option<Vec<JSXAttrOrSpread>> {
        if props_arg.spread.is_some() {
            return self.to_jsx_attrs_from_expr(props_arg.expr.as_ref());
        }
        self.to_jsx_attrs_from_expr(props_arg.expr.as_ref())
    }

    fn to_jsx_attrs_from_expr(&self, expr: &Expr) -> Option<Vec<JSXAttrOrSpread>> {
        match expr {
            Expr::Lit(Lit::Null(_)) => Some(Vec::new()),
            Expr::Call(call)
                if is_react_spread(call) || is_object_assign(call, self.unresolved_mark) =>
            {
                let mut attrs = Vec::new();
                for arg in &call.args {
                    attrs.extend(self.to_jsx_attrs(arg)?);
                }
                Some(attrs)
            }
            Expr::Object(obj) => Some(self.object_lit_to_jsx_attrs(obj)),
            _ => Some(vec![JSXAttrOrSpread::SpreadElement(SpreadElement {
                dot3_token: DUMMY_SP,
                expr: Box::new(expr.clone()),
            })]),
        }
    }

    fn object_lit_to_jsx_attrs(&self, obj: &ObjectLit) -> Vec<JSXAttrOrSpread> {
        let mut attrs = Vec::new();
        for prop in &obj.props {
            match prop {
                PropOrSpread::Spread(spread) => {
                    attrs.push(JSXAttrOrSpread::SpreadElement(SpreadElement {
                        dot3_token: DUMMY_SP,
                        expr: spread.expr.clone(),
                    }));
                }
                PropOrSpread::Prop(prop) => {
                    if let Some(attr) = self.prop_to_jsx_attr(prop.as_ref()) {
                        attrs.push(attr);
                    }
                }
            }
        }
        attrs
    }

    fn prop_to_jsx_attr(&self, prop: &Prop) -> Option<JSXAttrOrSpread> {
        match prop {
            Prop::KeyValue(KeyValueProp { key, value }) => {
                if is_computed_prop_name(key) {
                    return Some(wrap_prop_as_spread(prop.clone()));
                }
                let Some(name) = prop_name_to_attr_name(key) else {
                    return Some(wrap_prop_as_spread(prop.clone()));
                };
                if is_true_expr(value) {
                    return Some(JSXAttrOrSpread::JSXAttr(JSXAttr {
                        span: DUMMY_SP,
                        name,
                        value: None,
                    }));
                }
                Some(JSXAttrOrSpread::JSXAttr(JSXAttr {
                    span: DUMMY_SP,
                    name,
                    value: Some(self.expr_to_attr_value(value)),
                }))
            }
            Prop::Shorthand(ident) => Some(JSXAttrOrSpread::JSXAttr(JSXAttr {
                span: DUMMY_SP,
                name: prop_name_to_attr_name(&PropName::Ident(ident.clone().into()))?,
                value: Some(JSXAttrValue::JSXExprContainer(JSXExprContainer {
                    span: DUMMY_SP,
                    expr: JSXExpr::Expr(Box::new(Expr::Ident(ident.clone()))),
                })),
            })),
            Prop::Method(method) => {
                if is_computed_prop_name(&method.key) {
                    return Some(wrap_prop_as_spread(prop.clone()));
                }
                let Some(name) = prop_name_to_attr_name(&method.key) else {
                    return Some(wrap_prop_as_spread(prop.clone()));
                };
                let value = Expr::Fn(swc_core::ecma::ast::FnExpr {
                    ident: None,
                    function: method.function.clone(),
                });
                Some(JSXAttrOrSpread::JSXAttr(JSXAttr {
                    span: DUMMY_SP,
                    name,
                    value: Some(self.expr_to_attr_value(&value)),
                }))
            }
            _ => Some(wrap_prop_as_spread(prop.clone())),
        }
    }

    fn expr_to_attr_value(&self, expr: &Expr) -> JSXAttrValue {
        match expr {
            Expr::Lit(Lit::Str(s)) if can_string_be_attr_literal(s) => JSXAttrValue::Str(s.clone()),
            Expr::JSXElement(el) => JSXAttrValue::JSXElement(el.clone()),
            Expr::JSXFragment(fragment) => JSXAttrValue::JSXFragment(fragment.clone()),
            _ => JSXAttrValue::JSXExprContainer(JSXExprContainer {
                span: DUMMY_SP,
                expr: JSXExpr::Expr(Box::new(expr.clone())),
            }),
        }
    }

    fn jsx_attr_value_to_children(&self, value: JSXAttrValue) -> Vec<JSXElementChild> {
        match value {
            JSXAttrValue::Str(s) => self
                .expr_to_child(&Expr::Lit(Lit::Str(s)))
                .into_iter()
                .collect(),
            JSXAttrValue::JSXExprContainer(container) => match container.expr {
                JSXExpr::Expr(expr) => {
                    if let Expr::Array(array) = expr.as_ref() {
                        array
                            .elems
                            .iter()
                            .filter_map(|elem| elem.as_ref())
                            .filter_map(|elem| self.expr_or_spread_to_child(elem))
                            .collect()
                    } else {
                        self.expr_to_child(expr.as_ref()).into_iter().collect()
                    }
                }
                JSXExpr::JSXEmptyExpr(_) => Vec::new(),
            },
            JSXAttrValue::JSXElement(el) => vec![JSXElementChild::JSXElement(el)],
            JSXAttrValue::JSXFragment(fragment) => vec![JSXElementChild::JSXFragment(fragment)],
        }
    }

    fn expr_or_spread_to_child(&self, arg: &ExprOrSpread) -> Option<JSXElementChild> {
        if arg.spread.is_some() {
            return Some(JSXElementChild::JSXSpreadChild(JSXSpreadChild {
                span: DUMMY_SP,
                expr: arg.expr.clone(),
            }));
        }
        self.expr_to_child(arg.expr.as_ref())
    }

    fn expr_to_child(&self, expr: &Expr) -> Option<JSXElementChild> {
        match expr {
            Expr::JSXElement(el) => Some(JSXElementChild::JSXElement(el.clone())),
            Expr::JSXFragment(fragment) => Some(JSXElementChild::JSXFragment(fragment.clone())),
            Expr::Lit(Lit::Null(_)) => None,
            Expr::Lit(Lit::Bool(_)) => None,
            e if is_undefined_expr_boxed(e) => None,
            Expr::Lit(Lit::Str(s)) => string_child(s),
            _ => Some(JSXElementChild::JSXExprContainer(JSXExprContainer {
                span: DUMMY_SP,
                expr: JSXExpr::Expr(Box::new(expr.clone())),
            })),
        }
    }

    fn has_strong_jsx_shape(&self, pragma: &str, call: &CallExpr) -> bool {
        if call.args.len() < 2 {
            return false;
        }

        if expr_contains_inlined_jsx_component(call.args[0].expr.as_ref()) {
            return true;
        }

        let props_expr = call.args[1].expr.as_ref();
        if !expr_has_jsxish_props(props_expr, self.unresolved_mark) {
            return false;
        }

        is_automatic_pragma(pragma) || is_jsxish_props_container(props_expr, self.unresolved_mark)
    }

    /// Visit `expr` without converting its paren-transitive root call: the
    /// caller sits in a syntactic position where a JSX element cannot appear,
    /// and parens cannot rescue it because the fixer strips redundant ones.
    fn visit_keeping_root_call(&mut self, expr: &mut Expr) {
        let mut root: &mut Expr = expr;
        while let Expr::Paren(paren) = root {
            root = &mut paren.expr;
        }
        if matches!(root, Expr::Call(_)) {
            root.visit_mut_children_with(self);
        } else {
            root.visit_mut_with(self);
        }
    }
}

impl VisitMut for UnJsx {
    fn visit_mut_module(&mut self, module: &mut Module) {
        if self.level < RewriteLevel::Standard || !Self::should_run(module) {
            return;
        }
        self.process_module_items(&mut module.body);
    }

    fn visit_mut_block_stmt(&mut self, block: &mut BlockStmt) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        if stmts_have_jsx_content(&block.stmts, &self.import_pragmas) {
            self.process_stmts(&mut block.stmts, false);
        } else {
            block.visit_mut_children_with(self);
        }
    }

    // Function bodies are a distinct node from blocks; without this override
    // hoisted component aliases would land in the enclosing statement list
    // instead of the function that uses them.
    fn visit_mut_function_body(&mut self, body: &mut FunctionBody) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        if stmts_have_jsx_content(&body.stmts, &self.import_pragmas) {
            self.process_stmts(&mut body.stmts, true);
        } else {
            body.visit_mut_children_with(self);
        }
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        self.function_depth += 1;
        function.visit_mut_children_with(self);
        self.function_depth -= 1;
    }

    fn visit_mut_class(&mut self, class: &mut Class) {
        self.function_depth += 1;
        class.visit_mut_children_with(self);
        self.function_depth -= 1;
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        self.function_depth += 1;
        arrow.params.visit_mut_with(self);
        match arrow.body.as_mut() {
            ArrowFunctionBody::FunctionBody(body) => body.visit_mut_with(self),
            ArrowFunctionBody::Expr(expr) => {
                // An expression body has no statement list; open one for it
                // and, if an alias lands there, turn the body into a block so
                // the alias is evaluated on every call, in the arrow's scope.
                self.push_pending_frame();
                expr.visit_mut_with(self);
                let mut stmts = self.pop_pending_frame();
                if !stmts.is_empty() {
                    stmts.push(Stmt::Return(ReturnStmt {
                        span: DUMMY_SP,
                        arg: Some(expr.take()),
                    }));
                    *arrow.body = ArrowFunctionBody::FunctionBody(FunctionBody {
                        span: DUMMY_SP,
                        stmts,
                    });
                }
            }
        }
        self.function_depth -= 1;
    }

    fn visit_mut_member_expr(&mut self, member: &mut MemberExpr) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        // JSX cannot be emitted directly as a member object (`<X/>.type`).
        // Parens do not help: the fixer strips a redundant `(<X/>)` later.
        self.visit_keeping_root_call(&mut member.obj);
        member.prop.visit_mut_with(self);
    }

    fn visit_mut_new_expr(&mut self, new_expr: &mut NewExpr) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        // JSX cannot be a `new` callee (`new <X/>()`).
        self.visit_keeping_root_call(&mut new_expr.callee);
        new_expr.args.visit_mut_with(self);
    }

    fn visit_mut_callee(&mut self, callee: &mut Callee) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        // JSX cannot be called directly (`<X/>()`).
        match callee {
            Callee::Expr(expr) => self.visit_keeping_root_call(expr),
            _ => callee.visit_mut_children_with(self),
        }
    }

    fn visit_mut_opt_call(&mut self, call: &mut OptCall) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        // Optional calls carry their callee directly, not through `Callee`:
        // JSX cannot be optionally called either (`<X/>?.()`).
        self.visit_keeping_root_call(&mut call.callee);
        call.args.visit_mut_with(self);
    }

    fn visit_mut_tagged_tpl(&mut self, tagged: &mut TaggedTpl) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        // JSX cannot be a template tag (`<X/>\`\``).
        self.visit_keeping_root_call(&mut tagged.tag);
        tagged.tpl.visit_mut_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        expr.visit_mut_children_with(self);

        let replacement = match expr {
            Expr::Call(call) => self.convert_call(call),
            _ => None,
        };

        if let Some(replacement) = replacement {
            *expr = replacement;
        }
    }
}

fn stmts_have_jsx_content(
    stmts: &[Stmt],
    import_pragmas: &HashMap<BindingId, &'static str>,
) -> bool {
    struct BlockScan<'a> {
        found: bool,
        import_pragmas: &'a HashMap<BindingId, &'static str>,
    }
    impl Visit for BlockScan<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if self.found {
                return;
            }
            if let Callee::Expr(expr) = &call.callee {
                let is_pragma = match expr.as_ref() {
                    Expr::Ident(id) => {
                        is_jsx_pragma_name(id.sym.as_ref())
                            || self.import_pragmas.contains_key(&(id.sym.clone(), id.ctxt))
                    }
                    Expr::Member(m) => {
                        matches!(&m.prop, MemberProp::Ident(p) if is_jsx_pragma_name(p.sym.as_ref())
                            && !(p.sym.as_ref() == CLASSIC_PRAGMA
                                && matches!(m.obj.as_ref(), Expr::Ident(obj) if obj.sym.as_ref() == "document")))
                    }
                    _ => false,
                };
                if is_pragma {
                    self.found = true;
                    return;
                }
            }
            call.visit_children_with(self);
        }
        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if self.found {
                return;
            }
            if let swc_core::ecma::ast::AssignTarget::Simple(
                swc_core::ecma::ast::SimpleAssignTarget::Member(member),
            ) = &assign.left
            {
                if let MemberProp::Ident(prop) = &member.prop {
                    if prop.sym.as_ref() == "displayName" {
                        if let Expr::Ident(obj) = member.obj.as_ref() {
                            if obj.sym.len() <= 2 {
                                self.found = true;
                                return;
                            }
                        }
                    }
                }
            }
            assign.visit_children_with(self);
        }
    }
    let mut scan = BlockScan {
        found: false,
        import_pragmas,
    };
    stmts.visit_with(&mut scan);
    scan.found
}

#[derive(Default)]
struct NameCollector {
    names: HashSet<Atom>,
}

impl Visit for NameCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        self.names.insert(ident.sym.clone());
    }

    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.names.insert(ident.id.sym.clone());
    }
}

fn strip_unused_classic_pragma_imports(items: &mut [ModuleItem]) {
    let referenced = BindingUseIndex::collect_module_items(items).referenced_bindings();

    for item in items {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        import.specifiers.retain(|spec| match spec {
            ImportSpecifier::Default(default) if default.local.sym == *CLASSIC_PRAGMA => {
                referenced.contains(&(default.local.sym.clone(), default.local.ctxt))
            }
            ImportSpecifier::Named(named) if named.local.sym == *CLASSIC_PRAGMA => {
                referenced.contains(&(named.local.sym.clone(), named.local.ctxt))
            }
            _ => true,
        });
    }
}

#[derive(Default)]
struct ConstStringCollector {
    values: HashMap<BindingId, Str>,
}

impl Visit for ConstStringCollector {
    fn visit_var_decl(&mut self, var_decl: &VarDecl) {
        if var_decl.kind != VarDeclKind::Const {
            return;
        }
        for decl in &var_decl.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = &decl.init else {
                continue;
            };
            let value = match init.as_ref() {
                Expr::Lit(Lit::Str(value)) => value.clone(),
                Expr::Tpl(_) => match no_substitution_template_str(init) {
                    Some(value) => value,
                    None => continue,
                },
                _ => continue,
            };
            self.values
                .insert((binding.id.sym.clone(), binding.id.ctxt), value);
        }
    }
}

fn collect_names_in_module_items(items: &[ModuleItem]) -> HashSet<Atom> {
    let mut collector = NameCollector::default();
    items.visit_with(&mut collector);
    collector.names
}

fn collect_names_in_stmts(stmts: &[Stmt]) -> HashSet<Atom> {
    let mut collector = NameCollector::default();
    stmts.visit_with(&mut collector);
    collector.names
}

fn collect_string_consts_from_module_items(items: &[ModuleItem]) -> HashMap<BindingId, Str> {
    let mut collector = ConstStringCollector::default();
    items.visit_with(&mut collector);
    collector.values
}

fn collect_string_consts_from_stmts(stmts: &[Stmt]) -> HashMap<BindingId, Str> {
    let mut collector = ConstStringCollector::default();
    stmts.visit_with(&mut collector);
    collector.values
}

fn collect_import_pragmas(items: &[ModuleItem]) -> HashMap<BindingId, &'static str> {
    let mut map = HashMap::default();
    for item in items {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let src = wtf8_to_string(&import.src.value);
        if !matches!(src.as_str(), "react/jsx-runtime" | "react/jsx-dev-runtime") {
            continue;
        }
        for spec in &import.specifiers {
            let ImportSpecifier::Named(named) = spec else {
                continue;
            };
            let imported_name = match &named.imported {
                Some(ModuleExportName::Ident(id)) => id.sym.to_string(),
                Some(ModuleExportName::Str(s)) => wtf8_to_string(&s.value),
                None => continue,
            };
            let pragma: Option<&'static str> = match imported_name.as_str() {
                "jsx" => Some("jsx"),
                "jsxs" => Some("jsxs"),
                "jsxDEV" => Some("jsxDEV"),
                "jsxsDEV" => Some("jsxsDEV"),
                _ => None,
            };
            if let Some(pragma) = pragma {
                map.insert((named.local.sym.clone(), named.local.ctxt), pragma);
            }
        }
    }
    map
}

/// `x.displayName = "Name"` assignments that belong to a statement list's
/// scope: direct statements plus those nested in `try`, `if`, loop, and
/// labeled bodies. The scan stops at function and class boundaries because a
/// binding named there is renamed when that body is processed.
#[derive(Default)]
struct DisplayNameAssignScan {
    candidates: Vec<(Ident, Str)>,
}

impl Visit for DisplayNameAssignScan {
    fn visit_function(&mut self, _: &swc_core::ecma::ast::Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_class(&mut self, _: &swc_core::ecma::ast::Class) {}

    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Some(candidate) = display_name_assignment(stmt) {
            self.candidates.push(candidate);
        }
        stmt.visit_children_with(self);
    }
}

fn display_name_assignment(stmt: &Stmt) -> Option<(Ident, Str)> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left,
        right,
        ..
    }) = expr_stmt.expr.as_ref()
    else {
        return None;
    };
    let swc_core::ecma::ast::AssignTarget::Simple(simple) = left else {
        return None;
    };
    let swc_core::ecma::ast::SimpleAssignTarget::Member(member) = simple else {
        return None;
    };
    let Expr::Ident(object) = member.obj.as_ref() else {
        return None;
    };
    let MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    if prop.sym != *"displayName" || object.sym.len() > 2 {
        return None;
    }
    let Expr::Lit(Lit::Str(display_name)) = right.as_ref() else {
        return None;
    };
    Some((object.clone(), display_name.clone()))
}

fn collect_display_name_candidates_in_module_items(items: &[ModuleItem]) -> Vec<(Ident, Str)> {
    let mut scan = DisplayNameAssignScan::default();
    items.visit_with(&mut scan);
    scan.candidates
}

fn collect_display_name_candidates_in_stmts(stmts: &[Stmt]) -> Vec<(Ident, Str)> {
    let mut scan = DisplayNameAssignScan::default();
    stmts.visit_with(&mut scan);
    scan.candidates
}

/// Bindings a statement list may rename without leaving a reference behind:
/// every reference to them sits inside the list. Block-scoped declarations
/// (`let`, `const`, `class`) qualify at any depth; `var`, function
/// declarations, and parameters hoist to the nearest function, so at the
/// list's own level they qualify only when the list is a function body or the
/// module.
struct RenamableBindingCollector {
    ids: HashSet<BindingId>,
    function_depth: usize,
    list_is_function_scope: bool,
}

impl RenamableBindingCollector {
    fn hoisted_bindings_stay_inside(&self) -> bool {
        self.list_is_function_scope || self.function_depth > 0
    }

    fn add_pat(&mut self, pat: &Pat) {
        let ids: Vec<BindingId> = find_pat_ids(pat);
        self.ids.extend(ids);
    }
}

impl Visit for RenamableBindingCollector {
    fn visit_var_decl(&mut self, decl: &VarDecl) {
        if decl.kind != VarDeclKind::Var || self.hoisted_bindings_stay_inside() {
            for declarator in &decl.decls {
                self.add_pat(&declarator.name);
            }
        }
        decl.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
        if self.hoisted_bindings_stay_inside() {
            self.ids.insert((decl.ident.sym.clone(), decl.ident.ctxt));
        }
        decl.visit_children_with(self);
    }

    fn visit_class_decl(&mut self, decl: &swc_core::ecma::ast::ClassDecl) {
        self.ids.insert((decl.ident.sym.clone(), decl.ident.ctxt));
        decl.visit_children_with(self);
    }

    fn visit_import_decl(&mut self, decl: &ImportDecl) {
        for specifier in &decl.specifiers {
            let local = match specifier {
                ImportSpecifier::Named(named) => &named.local,
                ImportSpecifier::Default(default) => &default.local,
                ImportSpecifier::Namespace(namespace) => &namespace.local,
            };
            self.ids.insert((local.sym.clone(), local.ctxt));
        }
    }

    fn visit_catch_clause(&mut self, clause: &swc_core::ecma::ast::CatchClause) {
        if let Some(param) = &clause.param {
            self.add_pat(param);
        }
        clause.visit_children_with(self);
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        self.function_depth += 1;
        for param in &function.params {
            self.add_pat(&param.pat);
        }
        function.visit_children_with(self);
        self.function_depth -= 1;
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.function_depth += 1;
        for param in &arrow.params {
            self.add_pat(param);
        }
        arrow.visit_children_with(self);
        self.function_depth -= 1;
    }
}

fn collect_renamable_binding_ids(
    stmts: &[Stmt],
    list_is_function_scope: bool,
) -> HashSet<BindingId> {
    let mut collector = RenamableBindingCollector {
        ids: HashSet::default(),
        function_depth: 0,
        list_is_function_scope,
    };
    stmts.visit_with(&mut collector);
    collector.ids
}

fn has_lowercase_jsx_component_calls(
    items: &[ModuleItem],
    import_pragmas: &HashMap<BindingId, &'static str>,
    unresolved_mark: Mark,
) -> bool {
    struct Scan<'a> {
        found: bool,
        import_pragmas: &'a HashMap<BindingId, &'static str>,
        unresolved_mark: Mark,
    }
    impl Visit for Scan<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if self.found {
                return;
            }
            if get_pragma(&call.callee, self.import_pragmas).is_some() {
                if let Some(first) = call.args.first() {
                    if first.spread.is_none() {
                        if let Expr::Ident(id) = first.expr.as_ref() {
                            if starts_with_lowercase(id.sym.as_ref())
                                && id.ctxt.outer() != self.unresolved_mark
                            {
                                self.found = true;
                                return;
                            }
                        }
                    }
                }
            }
            call.visit_children_with(self);
        }
    }
    let mut scan = Scan {
        found: false,
        import_pragmas,
        unresolved_mark,
    };
    items.visit_with(&mut scan);
    scan.found
}

fn has_lowercase_jsx_component_calls_stmts(
    stmts: &[Stmt],
    import_pragmas: &HashMap<BindingId, &'static str>,
    unresolved_mark: Mark,
) -> bool {
    struct Scan<'a> {
        found: bool,
        import_pragmas: &'a HashMap<BindingId, &'static str>,
        unresolved_mark: Mark,
    }
    impl Visit for Scan<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if self.found {
                return;
            }
            if get_pragma(&call.callee, self.import_pragmas).is_some() {
                if let Some(first) = call.args.first() {
                    if first.spread.is_none() {
                        if let Expr::Ident(id) = first.expr.as_ref() {
                            if starts_with_lowercase(id.sym.as_ref())
                                && id.ctxt.outer() != self.unresolved_mark
                            {
                                self.found = true;
                                return;
                            }
                        }
                    }
                }
            }
            call.visit_children_with(self);
        }
    }
    let mut scan = Scan {
        found: false,
        import_pragmas,
        unresolved_mark,
    };
    stmts.visit_with(&mut scan);
    scan.found
}

fn collect_module_renames(
    items: &[ModuleItem],
    unresolved_mark: Mark,
    import_pragmas: &HashMap<BindingId, &'static str>,
) -> (Vec<BindingRename>, HashSet<Atom>) {
    let display_candidates = collect_display_name_candidates_in_module_items(items);
    let has_lc = has_lowercase_jsx_component_calls(items, import_pragmas, unresolved_mark);
    if display_candidates.is_empty() && !has_lc {
        return (Vec::new(), collect_names_in_module_items(items));
    }
    let mut name_registry = collect_names_in_module_items(items);
    let mut renames = display_name_renames(display_candidates, &mut name_registry);
    if has_lc {
        renames.extend(collect_lowercase_component_renames_from_module_items(
            items,
            unresolved_mark,
            &mut name_registry,
            import_pragmas,
        ));
    }
    let exported_bindings = collect_exported_binding_ids_from_items(items);
    renames.retain(|rename| !exported_bindings.contains(&rename.old));
    (renames, name_registry)
}

fn collect_stmt_renames(
    stmts: &[Stmt],
    unresolved_mark: Mark,
    import_pragmas: &HashMap<BindingId, &'static str>,
    list_is_function_scope: bool,
) -> (Vec<BindingRename>, HashSet<Atom>) {
    let display_candidates = collect_display_name_candidates_in_stmts(stmts);
    let has_lc = has_lowercase_jsx_component_calls_stmts(stmts, import_pragmas, unresolved_mark);
    if display_candidates.is_empty() && !has_lc {
        return (Vec::new(), collect_names_in_stmts(stmts));
    }
    let mut name_registry = collect_names_in_stmts(stmts);
    let mut renames = display_name_renames(display_candidates, &mut name_registry);
    if has_lc {
        renames.extend(collect_lowercase_component_renames_from_stmts(
            stmts,
            unresolved_mark,
            &mut name_registry,
            import_pragmas,
        ));
    }
    // The renamer only walks this list. A binding declared outside it (a
    // parameter, or a `let` of the enclosing body when this list is a nested
    // block) would keep its old name at the declaration and every other use.
    let renamable = collect_renamable_binding_ids(stmts, list_is_function_scope);
    renames.retain(|rename| renamable.contains(&rename.old));
    (renames, name_registry)
}

fn display_name_renames(
    candidates: Vec<(Ident, Str)>,
    used_names: &mut HashSet<Atom>,
) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    let mut seen = HashSet::default();
    for (object, display_name) in candidates {
        let old: BindingId = (object.sym.clone(), object.ctxt);
        if !seen.insert(old.clone()) {
            continue;
        }
        let new_name = generate_unique_name(
            used_names,
            to_valid_identifier_name(&pascalize(&wtf8_to_string(&display_name.value))),
        );
        renames.push(BindingRename {
            old,
            new: new_name.into(),
        });
    }
    renames
}

fn collect_lowercase_component_renames_from_module_items(
    items: &[ModuleItem],
    unresolved_mark: Mark,
    used_names: &mut HashSet<Atom>,
    import_pragmas: &HashMap<BindingId, &'static str>,
) -> Vec<BindingRename> {
    let eligible_bindings = collect_eligible_component_bindings_from_module_items(items);
    let mut visitor = LowercaseComponentRenameCollector {
        unresolved_mark,
        used_names,
        eligible_bindings,
        import_pragmas,
        renames: Vec::new(),
    };
    items.visit_with(&mut visitor);
    visitor.renames
}

fn collect_lowercase_component_renames_from_stmts(
    stmts: &[Stmt],
    unresolved_mark: Mark,
    used_names: &mut HashSet<Atom>,
    import_pragmas: &HashMap<BindingId, &'static str>,
) -> Vec<BindingRename> {
    let eligible_bindings = collect_eligible_component_bindings_from_stmts(stmts);
    let mut visitor = LowercaseComponentRenameCollector {
        unresolved_mark,
        used_names,
        eligible_bindings,
        import_pragmas,
        renames: Vec::new(),
    };
    stmts.visit_with(&mut visitor);
    visitor.renames
}

struct LowercaseComponentRenameCollector<'a> {
    unresolved_mark: Mark,
    used_names: &'a mut HashSet<Atom>,
    eligible_bindings: HashMap<BindingId, ComponentBindingEligibility>,
    import_pragmas: &'a HashMap<BindingId, &'static str>,
    renames: Vec<BindingRename>,
}

impl Visit for LowercaseComponentRenameCollector<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        let Some(pragma) = get_pragma(&call.callee, self.import_pragmas) else {
            call.visit_children_with(self);
            return;
        };

        if let Some(first) = call.args.first() {
            if first.spread.is_none() {
                if let Expr::Ident(ident) = first.expr.as_ref() {
                    if starts_with_lowercase(ident.sym.as_ref())
                        && ident.ctxt.outer() != self.unresolved_mark
                    {
                        let binding_id = (ident.sym.clone(), ident.ctxt);
                        if let Some(eligibility) = self.eligible_bindings.get(&binding_id) {
                            let can_rename = eligibility.allow_classic_create_element
                                || is_automatic_pragma(pragma);
                            if can_rename {
                                let target_name = eligibility
                                    .hint
                                    .clone()
                                    .unwrap_or_else(|| pascalize(ident.sym.as_ref()));
                                let new_name = generate_unique_name(self.used_names, target_name);
                                self.renames.push(BindingRename {
                                    old: binding_id,
                                    new: new_name.into(),
                                });
                            }
                        }
                    }
                }
            }
        }

        call.visit_children_with(self);
    }
}

#[derive(Default)]
struct EligibleComponentBindingCollector {
    bindings: HashMap<BindingId, ComponentBindingEligibility>,
    include_all_const_bindings: bool,
}

#[derive(Clone)]
struct ComponentBindingEligibility {
    hint: Option<String>,
    allow_classic_create_element: bool,
}

impl Visit for EligibleComponentBindingCollector {
    fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
        self.record(&decl.ident, None, true);
    }

    fn visit_class_decl(&mut self, decl: &swc_core::ecma::ast::ClassDecl) {
        self.record(&decl.ident, None, true);
    }

    fn visit_import_decl(&mut self, decl: &ImportDecl) {
        for specifier in &decl.specifiers {
            match specifier {
                ImportSpecifier::Named(named) => {
                    self.record(&named.local, None, true);
                }
                ImportSpecifier::Default(default) => {
                    self.record(&default.local, None, true);
                }
                ImportSpecifier::Namespace(namespace) => {
                    self.record(&namespace.local, None, true);
                }
            }
        }
    }

    fn visit_var_decl(&mut self, decl: &VarDecl) {
        if !matches!(decl.kind, VarDeclKind::Const | VarDeclKind::Var) {
            return;
        }

        for declarator in &decl.decls {
            if self.include_all_const_bindings {
                self.add_pat(&declarator.name, decl.kind == VarDeclKind::Const);
                continue;
            }
            if declarator.init.is_some() {
                if let Pat::Ident(binding) = &declarator.name {
                    let init = declarator.init.as_deref().expect("checked above");
                    let is_const = decl.kind == VarDeclKind::Const;
                    if !is_const && !is_likely_var_component_initializer(init) {
                        continue;
                    }
                    let hint = component_name_hint_from_expr(init);
                    self.record(&binding.id, hint, is_const);
                } else {
                    self.add_pat(&declarator.name, decl.kind == VarDeclKind::Const);
                }
            }
        }
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        for param in &function.params {
            self.add_param(param);
        }
        function.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        for param in &arrow.params {
            self.add_pat(param, true);
        }
        arrow.visit_children_with(self);
    }
}

impl EligibleComponentBindingCollector {
    fn record(&mut self, ident: &Ident, hint: Option<String>, allow_classic_create_element: bool) {
        self.bindings.insert(
            (ident.sym.clone(), ident.ctxt),
            ComponentBindingEligibility {
                hint,
                allow_classic_create_element,
            },
        );
    }

    fn add_param(&mut self, param: &Param) {
        self.add_pat(&param.pat, true);
    }

    fn add_pat(&mut self, pat: &Pat, allow_classic_create_element: bool) {
        match pat {
            Pat::Ident(binding) => {
                self.record(&binding.id, None, allow_classic_create_element);
            }
            Pat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.add_pat(elem, allow_classic_create_element);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                            self.record(&assign.key, None, allow_classic_create_element);
                        }
                        swc_core::ecma::ast::ObjectPatProp::KeyValue(key_value) => {
                            self.add_pat(&key_value.value, allow_classic_create_element);
                        }
                        swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                            self.add_pat(&rest.arg, allow_classic_create_element);
                        }
                    }
                }
            }
            Pat::Assign(assign) => self.add_pat(&assign.left, allow_classic_create_element),
            Pat::Rest(rest) => self.add_pat(&rest.arg, allow_classic_create_element),
            _ => {}
        }
    }
}

fn collect_eligible_component_bindings_from_module_items(
    items: &[ModuleItem],
) -> HashMap<BindingId, ComponentBindingEligibility> {
    let mut collector = EligibleComponentBindingCollector::default();
    items.visit_with(&mut collector);
    collector.bindings
}

fn collect_eligible_component_bindings_from_stmts(
    stmts: &[Stmt],
) -> HashMap<BindingId, ComponentBindingEligibility> {
    let mut collector = EligibleComponentBindingCollector::default();
    stmts.visit_with(&mut collector);
    collector.bindings
}

fn component_name_hint_from_expr(expr: &Expr) -> Option<String> {
    let Expr::Member(member) = expr else {
        return None;
    };
    let MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    let name = prop.sym.as_ref();
    starts_with_lowercase(name).then(|| pascalize(name))
}

fn is_likely_var_component_initializer(expr: &Expr) -> bool {
    match expr {
        Expr::Arrow(_) | Expr::Call(_) | Expr::Class(_) | Expr::Fn(_) | Expr::New(_) => true,
        Expr::Paren(paren) => is_likely_var_component_initializer(&paren.expr),
        Expr::Seq(seq) => seq
            .exprs
            .last()
            .is_some_and(|expr| is_likely_var_component_initializer(expr)),
        _ => false,
    }
}

fn get_pragma(
    callee: &Callee,
    import_pragmas: &HashMap<BindingId, &'static str>,
) -> Option<&'static str> {
    let Callee::Expr(expr) = callee else {
        return None;
    };
    match expr.as_ref() {
        Expr::Ident(ident) => {
            if let Some(pragma) = import_pragmas.get(&(ident.sym.clone(), ident.ctxt)) {
                return Some(*pragma);
            }
            match ident.sym.as_ref() {
                CLASSIC_PRAGMA => Some(CLASSIC_PRAGMA),
                "jsx" => Some("jsx"),
                "jsxs" => Some("jsxs"),
                "_jsx" => Some("_jsx"),
                "_jsxs" => Some("_jsxs"),
                "jsxDEV" => Some("jsxDEV"),
                "jsxsDEV" => Some("jsxsDEV"),
                _ => None,
            }
        }
        Expr::Member(member) => {
            let Expr::Ident(object) = member.obj.as_ref() else {
                return None;
            };
            let MemberProp::Ident(prop) = &member.prop else {
                return None;
            };
            if object.sym == *"document" && prop.sym == *"createElement" {
                return None;
            }
            match prop.sym.as_ref() {
                CLASSIC_PRAGMA => Some(CLASSIC_PRAGMA),
                "jsx" => Some("jsx"),
                "jsxs" => Some("jsxs"),
                "jsxDEV" => Some("jsxDEV"),
                "jsxsDEV" => Some("jsxsDEV"),
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_capitalization_invalid(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(Lit::Str(s)) => !starts_with_lowercase(&wtf8_to_string(&s.value)),
        Expr::Tpl(_) => no_substitution_template_str(expr)
            .is_some_and(|s| !starts_with_lowercase(&wtf8_to_string(&s.value))),
        Expr::Ident(ident) => starts_with_lowercase(ident.sym.as_ref()),
        _ => false,
    }
}

/// A template literal with no substitutions is a string tag spelled with
/// backticks (`` createElement(`div`, …) ``): the tag is that one cooked
/// segment. A template with substitutions, or one whose segment has no cooked
/// value (an invalid escape), is not a string.
fn no_substitution_template_str(expr: &Expr) -> Option<Str> {
    let Expr::Tpl(tpl) = expr else {
        return None;
    };
    if !tpl.exprs.is_empty() || tpl.quasis.len() != 1 {
        return None;
    }
    let cooked = tpl.quasis[0].cooked.as_ref()?;
    Some(Str {
        span: tpl.span,
        value: cooked.clone(),
        raw: None,
    })
}

/// The resolver gives a lowercase element name the unresolved mark (it is an
/// intrinsic tag, not a binding); a name built from a string follows suit.
fn jsx_name_from_string(value: &Str, unresolved_mark: Mark) -> Option<JSXElementName> {
    let value_string = wtf8_to_string(&value.value);
    // String tags must remain intrinsic names after printing. In particular,
    // a dot would turn the tag into a component member expression in JSX.
    if !starts_with_lowercase(&value_string) {
        return None;
    }
    if let Some((ns, name)) = value_string.split_once(':') {
        if !is_valid_jsx_identifier(ns) || !is_valid_jsx_identifier(name) {
            return None;
        }
        return Some(JSXElementName::JSXNamespacedName(JSXNamespacedName {
            span: DUMMY_SP,
            ns: ns.into(),
            name: name.into(),
        }));
    }
    if !is_valid_jsx_identifier(&value_string) {
        return None;
    }
    let ctxt = if value_string.starts_with(|c: char| c.is_ascii_lowercase()) {
        SyntaxContext::empty().apply_mark(unresolved_mark)
    } else {
        SyntaxContext::empty()
    };
    Some(JSXElementName::Ident(Ident::new(
        value_string.into(),
        DUMMY_SP,
        ctxt,
    )))
}

fn prop_name_to_attr_name(name: &PropName) -> Option<JSXAttrName> {
    match name {
        PropName::Ident(ident) => Some(JSXAttrName::Ident(ident.clone())),
        PropName::Str(str_lit) => {
            let value = wtf8_to_string(&str_lit.value);
            if let Some((ns, name)) = value.split_once(':') {
                if !is_valid_jsx_identifier(ns) || !is_valid_jsx_identifier(name) {
                    return None;
                }
                Some(JSXAttrName::JSXNamespacedName(JSXNamespacedName {
                    span: DUMMY_SP,
                    ns: ns.into(),
                    name: name.into(),
                }))
            } else {
                if !is_valid_jsx_identifier(&value) {
                    return None;
                }
                Some(JSXAttrName::Ident(value.into()))
            }
        }
        _ => None,
    }
}

fn is_valid_jsx_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(Ident::is_valid_start)
        && chars.all(|character| character == '-' || Ident::is_valid_continue(character))
}

fn is_fragment_name(name: &JSXElementName) -> bool {
    match name {
        JSXElementName::Ident(ident) => ident.sym == *"Fragment",
        JSXElementName::JSXMemberExpr(member) => member.prop.sym == *"Fragment",
        _ => false,
    }
}

fn wrap_prop_as_spread(prop: Prop) -> JSXAttrOrSpread {
    JSXAttrOrSpread::SpreadElement(SpreadElement {
        dot3_token: DUMMY_SP,
        expr: Box::new(Expr::Object(ObjectLit {
            span: DUMMY_SP,
            props: vec![PropOrSpread::Prop(Box::new(prop))],
        })),
    })
}

fn is_react_spread(call: &CallExpr) -> bool {
    let Callee::Expr(expr) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = expr.as_ref() else {
        return false;
    };
    let Expr::Ident(_) = member.obj.as_ref() else {
        return false;
    };
    matches!(&member.prop, MemberProp::Ident(ident) if ident.sym == *"__spread")
}

fn is_object_assign(call: &CallExpr, unresolved_mark: Mark) -> bool {
    let Callee::Expr(expr) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = expr.as_ref() else {
        return false;
    };
    let Expr::Ident(object) = member.obj.as_ref() else {
        return false;
    };
    super::expr_utils::is_unresolved_ident(object, "Object", unresolved_mark)
        && matches!(&member.prop, MemberProp::Ident(ident) if ident.sym == *"assign")
}

fn is_computed_prop_name(name: &PropName) -> bool {
    matches!(name, PropName::Computed(_))
}

fn is_true_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Bool(Bool { value: true, .. })))
}

fn can_string_be_attr_literal(value: &Str) -> bool {
    let raw = value
        .raw
        .as_ref()
        .map(|raw| raw.as_ref())
        .unwrap_or_default();
    !raw.contains('\\')
        && value
            .value
            .as_str()
            .is_some_and(|value| !value.contains('"'))
}

fn string_child(value: &Str) -> Option<JSXElementChild> {
    // A lone surrogate has no UTF-8 form, so it cannot become JSX text
    // without changing the string; keep the literal in a container.
    let Some(text) = value.value.as_str().map(ToOwned::to_owned) else {
        return Some(JSXElementChild::JSXExprContainer(JSXExprContainer {
            span: DUMMY_SP,
            expr: JSXExpr::Expr(Box::new(Expr::Lit(Lit::Str(value.clone())))),
        }));
    };
    if text.is_empty() {
        return Some(JSXElementChild::JSXExprContainer(JSXExprContainer {
            span: DUMMY_SP,
            expr: JSXExpr::Expr(Box::new(Expr::Lit(Lit::Str(value.clone())))),
        }));
    }
    // Only characters that are syntactically invalid inside JSXText need wrapping.
    // Leading/trailing whitespace is preserved by JSXText on a single line.
    let needs_expr =
        text.contains(['{', '}', '<', '>']) || text.contains('\r') || text.contains('\n');
    if needs_expr {
        return Some(JSXElementChild::JSXExprContainer(JSXExprContainer {
            span: DUMMY_SP,
            expr: JSXExpr::Expr(Box::new(Expr::Lit(Lit::Str(value.clone())))),
        }));
    }
    Some(JSXElementChild::JSXText(JSXText {
        span: DUMMY_SP,
        value: text.clone().into(),
        raw: text.into(),
    }))
}

fn is_undefined_expr(expr: &Box<Expr>) -> bool {
    is_undefined_expr_boxed(expr.as_ref())
}

fn is_undefined_expr_boxed(expr: &Expr) -> bool {
    matches!(expr, Expr::Ident(ident) if ident.sym == *"undefined")
        || matches!(
            expr,
            Expr::Unary(unary)
                if unary.op == swc_core::ecma::ast::UnaryOp::Void
                    && matches!(
                        unary.arg.as_ref(),
                        Expr::Lit(Lit::Num(Number { value, .. })) if (*value - 0.0).abs() < f64::EPSILON
                    )
        )
}

fn extract_children_attr(attrs: &mut Vec<JSXAttrOrSpread>) -> Option<JSXAttrValue> {
    let idx = attrs.iter().position(|attr| {
        matches!(
            attr,
            JSXAttrOrSpread::JSXAttr(JSXAttr {
                name: JSXAttrName::Ident(name),
                ..
            }) if name.sym == *"children"
        )
    })?;
    let JSXAttrOrSpread::JSXAttr(attr) = attrs.remove(idx) else {
        return None;
    };
    attr.value
}

fn expr_has_jsxish_props(expr: &Expr, unresolved_mark: Mark) -> bool {
    match expr {
        Expr::Object(obj) => object_lit_has_jsxish_props(obj, unresolved_mark),
        Expr::Call(call) if is_react_spread(call) || is_object_assign(call, unresolved_mark) => {
            call.args
                .iter()
                .any(|arg| expr_has_jsxish_props(arg.expr.as_ref(), unresolved_mark))
        }
        _ => false,
    }
}

fn is_jsxish_props_container(expr: &Expr, unresolved_mark: Mark) -> bool {
    matches!(expr, Expr::Object(_))
        || matches!(expr, Expr::Call(call) if is_react_spread(call) || is_object_assign(call, unresolved_mark))
}

fn object_lit_has_jsxish_props(obj: &ObjectLit, unresolved_mark: Mark) -> bool {
    obj.props.iter().any(|prop| match prop {
        PropOrSpread::Spread(spread) => {
            expr_has_jsxish_props(spread.expr.as_ref(), unresolved_mark)
        }
        PropOrSpread::Prop(prop) => prop_has_jsxish_name(prop.as_ref()),
    })
}

fn prop_has_jsxish_name(prop: &Prop) -> bool {
    match prop {
        Prop::KeyValue(KeyValueProp { key, .. }) => prop_name_is_jsxish(key),
        Prop::Method(method) => prop_name_is_jsxish(&method.key),
        Prop::Shorthand(ident) => is_jsxish_prop_name(ident.sym.as_ref()),
        _ => false,
    }
}

fn prop_name_is_jsxish(name: &PropName) -> bool {
    match name {
        PropName::Ident(ident) => is_jsxish_prop_name(ident.sym.as_ref()),
        PropName::Str(str_lit) => is_jsxish_prop_name(&wtf8_to_string(&str_lit.value)),
        _ => false,
    }
}

fn is_jsxish_prop_name(name: &str) -> bool {
    matches!(
        name,
        "children"
            | "className"
            | "style"
            | "ref"
            | "key"
            | "htmlFor"
            | "dangerouslySetInnerHTML"
            | "suppressHydrationWarning"
    ) || name
        .strip_prefix("on")
        .and_then(|rest| rest.chars().next())
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

fn expr_contains_inlined_jsx_component(expr: &Expr) -> bool {
    let mut visitor = JsxPresenceVisitor::default();
    match expr {
        Expr::Arrow(arrow) => arrow.body.visit_with(&mut visitor),
        Expr::Fn(function) => {
            if let Some(body) = &function.function.body {
                body.visit_with(&mut visitor);
            }
        }
        _ => return false,
    }
    visitor.found
}

#[derive(Default)]
struct JsxPresenceVisitor {
    found: bool,
}

impl Visit for JsxPresenceVisitor {
    fn visit_jsx_element(&mut self, _element: &JSXElement) {
        self.found = true;
    }

    fn visit_jsx_fragment(&mut self, _fragment: &JSXFragment) {
        self.found = true;
    }

    fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}

    fn visit_function(&mut self, _function: &swc_core::ecma::ast::Function) {}
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

fn generate_unique_name(used_names: &mut HashSet<Atom>, base: String) -> String {
    let base_atom = Atom::from(base.as_str());
    if !used_names.contains(&base_atom) {
        used_names.insert(base_atom);
        return base;
    }
    let mut idx = 1usize;
    loop {
        let candidate = format!("{base}{idx}");
        let candidate_atom = Atom::from(candidate.as_str());
        if !used_names.contains(&candidate_atom) {
            used_names.insert(candidate_atom);
            return candidate;
        }
        idx += 1;
    }
}

fn wtf8_to_string(value: &Wtf8Atom) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, Globals, Mark, SourceMap, GLOBALS};
    use swc_core::ecma::transforms::base::resolver;

    use super::*;

    #[derive(Default)]
    struct TagNames(Vec<Ident>);

    impl Visit for TagNames {
        fn visit_jsx_element_name(&mut self, name: &JSXElementName) {
            if let JSXElementName::Ident(ident) = name {
                self.0.push(ident.clone());
            }
        }
    }

    #[test]
    fn intrinsic_tag_built_from_a_string_carries_the_unresolved_mark() {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let source = r#"
import React from "react";
const el = React.createElement("div", null, "hi");
"#;
            let mut module = crate::unpacker::parse_es_module(source, "fixture.js", cm)
                .expect("fixture should parse");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));

            module.visit_mut_with(&mut UnJsx::new(unresolved_mark));

            let mut tags = TagNames::default();
            module.visit_with(&mut tags);
            // Opening and closing tag both carry the name.
            assert_eq!(
                tags.0.len(),
                2,
                "expected one JSX element, got {:?}",
                tags.0
            );
            for div in &tags.0 {
                assert_eq!(div.sym.as_ref(), "div");
                assert_eq!(div.ctxt.outer(), unresolved_mark);
            }
        });
    }
}
