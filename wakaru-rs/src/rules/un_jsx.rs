use std::collections::{HashMap, HashSet};

use swc_core::atoms::{Atom, Wtf8Atom};
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, BindingIdent, BlockStmt, Bool, Callee, CallExpr, Decl, Expr,
    ArrowExpr, ExprOrSpread, Ident, JSXAttr, JSXAttrName, JSXAttrOrSpread,
    JSXAttrValue, JSXClosingElement, JSXClosingFragment, JSXElement, JSXElementChild,
    JSXElementName, JSXExpr, JSXExprContainer, JSXFragment, JSXMemberExpr, JSXNamespacedName,
    JSXObject, JSXOpeningElement, JSXOpeningFragment, JSXSpreadChild, JSXText, KeyValueProp, Lit,
    MemberExpr, MemberProp, Module, ModuleItem, Number, ObjectLit, Pat, Prop, ImportDecl,
    ImportSpecifier, Param,
    PropName, PropOrSpread, SpreadElement, Stmt, Str, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::{RewriteLevel, Rule};

const CLASSIC_PRAGMA: &str = "createElement";

fn is_automatic_pragma(name: &str) -> bool {
    matches!(name, "jsx" | "jsxs" | "_jsx" | "_jsxs" | "jsxDEV" | "jsxsDEV")
}

type BindingId = (Atom, SyntaxContext);

#[derive(Clone)]
struct ScopedRename {
    old: BindingId,
    new: Atom,
}

pub struct UnJsx {
    unresolved_mark: Mark,
    level: RewriteLevel,
    pending_stmts: Vec<Vec<Stmt>>,
    used_names: Vec<HashSet<String>>,
    string_consts: Vec<HashMap<BindingId, Str>>,
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
            used_names: Vec::new(),
            string_consts: Vec::new(),
        }
    }

    fn process_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        let renames = collect_module_renames(items, self.unresolved_mark);
        if !renames.is_empty() {
            let mut renamer = ScopedRenamer::new(renames);
            for item in items.iter_mut() {
                item.visit_mut_with(&mut renamer);
            }
        }

        self.used_names.push(collect_names_in_module_items(items));
        self.string_consts.push(collect_string_consts_from_module_items(items));

        let old = std::mem::take(items);
        let mut rewritten = Vec::with_capacity(old.len());
        for mut item in old {
            self.pending_stmts.push(Vec::new());
            item.visit_mut_with(self);
            let pending = self.pending_stmts.pop().unwrap();
            rewritten.extend(pending.into_iter().map(ModuleItem::Stmt));
            rewritten.push(item);
        }

        self.string_consts.pop();
        self.used_names.pop();
        *items = rewritten;
    }

    fn process_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        let renames = collect_stmt_renames(stmts, self.unresolved_mark);
        if !renames.is_empty() {
            let mut renamer = ScopedRenamer::new(renames);
            for stmt in stmts.iter_mut() {
                stmt.visit_mut_with(&mut renamer);
            }
        }

        self.used_names.push(collect_names_in_stmts(stmts));
        self.string_consts.push(collect_string_consts_from_stmts(stmts));

        let old = std::mem::take(stmts);
        let mut rewritten = Vec::with_capacity(old.len());
        for mut stmt in old {
            self.pending_stmts.push(Vec::new());
            stmt.visit_mut_with(self);
            let pending = self.pending_stmts.pop().unwrap();
            rewritten.extend(pending);
            rewritten.push(stmt);
        }

        self.string_consts.pop();
        self.used_names.pop();
        *stmts = rewritten;
    }

    fn convert_call(&mut self, call: &CallExpr) -> Option<Expr> {
        let pragma = get_pragma(&call.callee)?;
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
            tag = self.to_jsx_element_name(&Expr::Lit(Lit::Str(inlined)));
        }

        if tag.is_none() && self.level >= RewriteLevel::Aggressive {
            let alias = self.create_component_alias(type_expr);
            tag = Some(JSXElementName::Ident(alias));
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
            return Some(Expr::JSXFragment(JSXFragment {
                span: DUMMY_SP,
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

        Some(Expr::JSXElement(Box::new(JSXElement {
            span: DUMMY_SP,
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

    fn create_component_alias(&mut self, expr: &Expr) -> Ident {
        let base = "Component".to_string();
        let name = self.generate_name(base);
        let ident = Ident::new(name.clone().into(), DUMMY_SP, SyntaxContext::empty());
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
        ident
    }

    fn generate_name(&mut self, base: String) -> String {
        let names = self.used_names.last_mut().expect("body scope should exist");
        if !names.contains(&base) {
            names.insert(base.clone());
            return base;
        }

        let mut idx = 1usize;
        loop {
            let candidate = format!("{base}_{idx}");
            if !names.contains(&candidate) {
                names.insert(candidate.clone());
                return candidate;
            }
            idx += 1;
        }
    }

    fn to_jsx_element_name(&self, expr: &Expr) -> Option<JSXElementName> {
        match expr {
            Expr::Lit(Lit::Str(s)) => jsx_name_from_string(s),
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
        if let Some(_) = props_arg.spread {
            return self.to_jsx_attrs_from_expr(props_arg.expr.as_ref());
        }
        self.to_jsx_attrs_from_expr(props_arg.expr.as_ref())
    }

    fn to_jsx_attrs_from_expr(&self, expr: &Expr) -> Option<Vec<JSXAttrOrSpread>> {
        match expr {
            Expr::Lit(Lit::Null(_)) => Some(Vec::new()),
            Expr::Call(call) if is_react_spread(call) || is_object_assign(call) => {
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
                let name = prop_name_to_attr_name(key)?;
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
                let name = prop_name_to_attr_name(&method.key)?;
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
}

impl VisitMut for UnJsx {
    fn visit_mut_module(&mut self, module: &mut Module) {
        self.process_module_items(&mut module.body);
    }

    fn visit_mut_block_stmt(&mut self, block: &mut BlockStmt) {
        self.process_stmts(&mut block.stmts);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
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

impl Rule for UnJsx {
    fn name(&self) -> &'static str {
        "un-jsx"
    }
}

#[derive(Default)]
struct NameCollector {
    names: HashSet<String>,
}

impl Visit for NameCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        self.names.insert(ident.sym.to_string());
    }

    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.names.insert(ident.id.sym.to_string());
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
            let Expr::Lit(Lit::Str(value)) = init.as_ref() else {
                continue;
            };
            self.values
                .insert((binding.id.sym.clone(), binding.id.ctxt), value.clone());
        }
    }
}

fn collect_names_in_module_items(items: &[ModuleItem]) -> HashSet<String> {
    let mut collector = NameCollector::default();
    items.visit_with(&mut collector);
    collector.names
}

fn collect_names_in_stmts(stmts: &[Stmt]) -> HashSet<String> {
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

fn collect_module_renames(items: &[ModuleItem], unresolved_mark: Mark) -> Vec<ScopedRename> {
    let used_names = collect_names_in_module_items(items);
    let mut name_registry = used_names;
    let mut renames = collect_display_name_renames_from_module_items(items, &mut name_registry);
    renames.extend(collect_lowercase_component_renames_from_module_items(
        items,
        unresolved_mark,
        &mut name_registry,
    ));
    renames
}

fn collect_stmt_renames(stmts: &[Stmt], unresolved_mark: Mark) -> Vec<ScopedRename> {
    let used_names = collect_names_in_stmts(stmts);
    let mut name_registry = used_names;
    let mut renames = collect_display_name_renames_from_stmts(stmts, &mut name_registry);
    renames.extend(collect_lowercase_component_renames_from_stmts(
        stmts,
        unresolved_mark,
        &mut name_registry,
    ));
    renames
}

fn collect_display_name_renames_from_module_items(
    items: &[ModuleItem],
    used_names: &mut HashSet<String>,
) -> Vec<ScopedRename> {
    let mut renames = Vec::new();
    for item in items {
        let ModuleItem::Stmt(stmt) = item else {
            continue;
        };
        collect_display_name_renames_from_stmt(stmt, used_names, &mut renames);
    }
    renames
}

fn collect_display_name_renames_from_stmts(
    stmts: &[Stmt],
    used_names: &mut HashSet<String>,
) -> Vec<ScopedRename> {
    let mut renames = Vec::new();
    for stmt in stmts {
        collect_display_name_renames_from_stmt(stmt, used_names, &mut renames);
    }
    renames
}

fn collect_display_name_renames_from_stmt(
    stmt: &Stmt,
    used_names: &mut HashSet<String>,
    renames: &mut Vec<ScopedRename>,
) {
    let Stmt::Expr(expr_stmt) = stmt else {
        return;
    };
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left,
        right,
        ..
    }) = expr_stmt.expr.as_ref()
    else {
        return;
    };
    let swc_core::ecma::ast::AssignTarget::Simple(simple) = left else {
        return;
    };
    let swc_core::ecma::ast::SimpleAssignTarget::Member(member) = simple else {
        return;
    };
    let Expr::Ident(object) = member.obj.as_ref() else {
        return;
    };
    let MemberProp::Ident(prop) = &member.prop else {
        return;
    };
    if prop.sym != *"displayName" || object.sym.len() > 2 {
        return;
    }
    let Expr::Lit(Lit::Str(display_name)) = right.as_ref() else {
        return;
    };
    let new_name = generate_unique_name(used_names, pascalize(&wtf8_to_string(&display_name.value)));
    renames.push(ScopedRename {
        old: (object.sym.clone(), object.ctxt),
        new: new_name.into(),
    });
}

fn collect_lowercase_component_renames_from_module_items(
    items: &[ModuleItem],
    unresolved_mark: Mark,
    used_names: &mut HashSet<String>,
) -> Vec<ScopedRename> {
    let eligible_bindings = collect_eligible_component_bindings_from_module_items(items);
    let mut visitor = LowercaseComponentRenameCollector {
        unresolved_mark,
        used_names,
        eligible_bindings,
        renames: Vec::new(),
    };
    items.visit_with(&mut visitor);
    visitor.renames
}

fn collect_lowercase_component_renames_from_stmts(
    stmts: &[Stmt],
    unresolved_mark: Mark,
    used_names: &mut HashSet<String>,
) -> Vec<ScopedRename> {
    let eligible_bindings = collect_eligible_component_bindings_from_stmts(stmts);
    let mut visitor = LowercaseComponentRenameCollector {
        unresolved_mark,
        used_names,
        eligible_bindings,
        renames: Vec::new(),
    };
    stmts.visit_with(&mut visitor);
    visitor.renames
}

struct LowercaseComponentRenameCollector<'a> {
    unresolved_mark: Mark,
    used_names: &'a mut HashSet<String>,
    eligible_bindings: HashSet<BindingId>,
    renames: Vec<ScopedRename>,
}

impl Visit for LowercaseComponentRenameCollector<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        let Some(_) = get_pragma(&call.callee) else {
            call.visit_children_with(self);
            return;
        };

        if let Some(first) = call.args.first() {
            if first.spread.is_none() {
                if let Expr::Ident(ident) = first.expr.as_ref() {
                    if starts_with_lowercase(ident.sym.as_ref())
                        && ident.ctxt.outer() != self.unresolved_mark
                        && self
                            .eligible_bindings
                            .contains(&(ident.sym.clone(), ident.ctxt))
                    {
                        let new_name = generate_unique_name(
                            self.used_names,
                            pascalize(ident.sym.as_ref()),
                        );
                        self.renames.push(ScopedRename {
                            old: (ident.sym.clone(), ident.ctxt),
                            new: new_name.into(),
                        });
                    }
                }
            }
        }

        call.visit_children_with(self);
    }
}

struct ScopedRenamer {
    renames: Vec<ScopedRename>,
}

impl ScopedRenamer {
    fn new(renames: Vec<ScopedRename>) -> Self {
        Self { renames }
    }
}

impl VisitMut for ScopedRenamer {
    fn visit_mut_ident(&mut self, ident: &mut Ident) {
        for rename in &self.renames {
            if ident.sym == rename.old.0 && ident.ctxt == rename.old.1 {
                ident.sym = rename.new.clone();
                break;
            }
        }
    }
}

#[derive(Default)]
struct EligibleComponentBindingCollector {
    bindings: HashSet<BindingId>,
    include_all_const_bindings: bool,
}

impl Visit for EligibleComponentBindingCollector {
    fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
        self.bindings.insert((decl.ident.sym.clone(), decl.ident.ctxt));
    }

    fn visit_class_decl(&mut self, decl: &swc_core::ecma::ast::ClassDecl) {
        self.bindings.insert((decl.ident.sym.clone(), decl.ident.ctxt));
    }

    fn visit_import_decl(&mut self, decl: &ImportDecl) {
        for specifier in &decl.specifiers {
            match specifier {
                ImportSpecifier::Named(named) => {
                    self.bindings
                        .insert((named.local.sym.clone(), named.local.ctxt));
                }
                ImportSpecifier::Default(default) => {
                    self.bindings
                        .insert((default.local.sym.clone(), default.local.ctxt));
                }
                ImportSpecifier::Namespace(namespace) => {
                    self.bindings
                        .insert((namespace.local.sym.clone(), namespace.local.ctxt));
                }
            }
        }
    }

    fn visit_var_decl(&mut self, decl: &VarDecl) {
        if decl.kind != VarDeclKind::Const {
            return;
        }

        for declarator in &decl.decls {
            if self.include_all_const_bindings {
                self.add_pat(&declarator.name);
                continue;
            }
            if declarator.init.is_some() {
                self.add_pat(&declarator.name);
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
            self.add_pat(param);
        }
        arrow.visit_children_with(self);
    }
}

impl EligibleComponentBindingCollector {
    fn add_param(&mut self, param: &Param) {
        self.add_pat(&param.pat);
    }

    fn add_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(binding) => {
                self.bindings
                    .insert((binding.id.sym.clone(), binding.id.ctxt));
            }
            Pat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.add_pat(elem);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                            self.bindings.insert((assign.key.sym.clone(), assign.key.ctxt));
                        }
                        swc_core::ecma::ast::ObjectPatProp::KeyValue(key_value) => {
                            self.add_pat(&key_value.value);
                        }
                        swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                            self.add_pat(&rest.arg);
                        }
                    }
                }
            }
            Pat::Assign(assign) => self.add_pat(&assign.left),
            Pat::Rest(rest) => self.add_pat(&rest.arg),
            _ => {}
        }
    }
}

fn collect_eligible_component_bindings_from_module_items(
    items: &[ModuleItem],
) -> HashSet<BindingId> {
    let mut collector = EligibleComponentBindingCollector::default();
    items.visit_with(&mut collector);
    collector.bindings
}

fn collect_eligible_component_bindings_from_stmts(stmts: &[Stmt]) -> HashSet<BindingId> {
    let mut collector = EligibleComponentBindingCollector::default();
    stmts.visit_with(&mut collector);
    collector.bindings
}

fn get_pragma(callee: &Callee) -> Option<&'static str> {
    let Callee::Expr(expr) = callee else {
        return None;
    };
    match expr.as_ref() {
        Expr::Ident(ident) => match ident.sym.as_ref() {
            CLASSIC_PRAGMA => Some(CLASSIC_PRAGMA),
            "jsx" => Some("jsx"),
            "jsxs" => Some("jsxs"),
            "_jsx" => Some("_jsx"),
            "_jsxs" => Some("_jsxs"),
            "jsxDEV" => Some("jsxDEV"),
            "jsxsDEV" => Some("jsxsDEV"),
            _ => None,
        },
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
        Expr::Ident(ident) => starts_with_lowercase(ident.sym.as_ref()),
        _ => false,
    }
}

fn starts_with_lowercase(value: &str) -> bool {
    value
        .chars()
        .next()
        .map(|ch| ch.is_ascii_lowercase())
        .unwrap_or(false)
}

fn jsx_name_from_string(value: &Str) -> Option<JSXElementName> {
    let value_string = wtf8_to_string(&value.value);
    if let Some((ns, name)) = value_string.split_once(':') {
        return Some(JSXElementName::JSXNamespacedName(JSXNamespacedName {
            span: DUMMY_SP,
            ns: ns.into(),
            name: name.into(),
        }));
    }
    Some(JSXElementName::Ident(Ident::new(
        value_string.into(),
        DUMMY_SP,
        SyntaxContext::empty(),
    )))
}

fn prop_name_to_attr_name(name: &PropName) -> Option<JSXAttrName> {
    match name {
        PropName::Ident(ident) => Some(JSXAttrName::Ident(ident.clone())),
        PropName::Str(str_lit) => {
            let value = wtf8_to_string(&str_lit.value);
            if let Some((ns, name)) = value.split_once(':') {
                Some(JSXAttrName::JSXNamespacedName(JSXNamespacedName {
                    span: DUMMY_SP,
                    ns: ns.into(),
                    name: name.into(),
                }))
            } else {
                Some(JSXAttrName::Ident(value.into()))
            }
        }
        _ => None,
    }
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

fn is_object_assign(call: &CallExpr) -> bool {
    let Callee::Expr(expr) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = expr.as_ref() else {
        return false;
    };
    let Expr::Ident(object) = member.obj.as_ref() else {
        return false;
    };
    object.sym == *"Object"
        && matches!(&member.prop, MemberProp::Ident(ident) if ident.sym == *"assign")
}

fn is_computed_prop_name(name: &PropName) -> bool {
    matches!(name, PropName::Computed(_))
}

fn is_true_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Bool(Bool { value: true, .. })))
}

fn can_string_be_attr_literal(value: &Str) -> bool {
    let raw = value.raw.as_ref().map(|raw| raw.as_ref()).unwrap_or_default();
    !raw.contains('\\') && !wtf8_to_string(&value.value).contains('"')
}

fn string_child(value: &Str) -> Option<JSXElementChild> {
    let text = wtf8_to_string(&value.value);
    if text.is_empty() {
        return Some(JSXElementChild::JSXExprContainer(JSXExprContainer {
            span: DUMMY_SP,
            expr: JSXExpr::Expr(Box::new(Expr::Lit(Lit::Str(value.clone())))),
        }));
    }
    let needs_expr = text.contains(['{', '}', '<', '>', '\r', '\n'])
        || text.starts_with(char::is_whitespace)
        || text.ends_with(char::is_whitespace);
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

fn generate_unique_name(used_names: &mut HashSet<String>, base: String) -> String {
    if !used_names.contains(&base) {
        used_names.insert(base.clone());
        return base;
    }
    let mut idx = 1usize;
    loop {
        let candidate = format!("{base}{idx}");
        if !used_names.contains(&candidate) {
            used_names.insert(candidate.clone());
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
