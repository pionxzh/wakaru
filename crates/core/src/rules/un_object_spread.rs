use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    BlockStmtOrExpr, CallExpr, Callee, Decl, Expr, Function, Ident, ImportSpecifier, Module,
    ModuleDecl, ModuleItem, ObjectLit, Pat, Prop, PropName, PropOrSpread, SpreadElement, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::facts::{HelperKind, ModuleFactsMap};

use super::babel_helper_utils::{
    collect_helper_dependencies, helpers_with_remaining_refs, remove_helper_declarations,
    tslib_member_helper_kind, BabelHelperKind, BindingKey, LocalHelperContext,
};

use crate::utils::paren::strip_parens;

/// Detects and replaces `_extends` and `_objectSpread2` helper calls with
/// object spread syntax.
///
/// Both `_extends` and `_objectSpread2` mutate and return their first argument
/// (like Object.assign). Only transform when the first arg is a safe fresh object
/// literal target, which guarantees no mutation/identity side effects:
///   `_extends({}, obj1, obj2)` → `{ ...obj1, ...obj2 }`
///   `_extends({ a: 1 }, obj1)` → `{ a: 1, ...obj1 }`
///   `_objectSpread2({}, y)` → `{ ...y }`
///   `_extends(target, source)` → left as-is (mutation semantics)
///   `_objectSpread2(existing, {a: 1})` → left as-is (mutation semantics)
pub struct UnObjectSpread<'a> {
    module_facts: Option<&'a ModuleFactsMap>,
}

impl UnObjectSpread<'_> {
    pub fn new() -> Self {
        Self { module_facts: None }
    }
}

impl<'a> UnObjectSpread<'a> {
    pub fn new_with_facts(module_facts: &'a ModuleFactsMap) -> Self {
        Self {
            module_facts: Some(module_facts),
        }
    }

    pub(crate) fn run_with_helpers(
        module: &mut Module,
        local_helpers: &LocalHelperContext,
        module_facts: Option<&ModuleFactsMap>,
    ) {
        run_un_object_spread(module, local_helpers, module_facts);
    }
}

impl VisitMut for UnObjectSpread<'_> {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_object_spread(module, &local_helpers, self.module_facts);
    }
}

fn run_un_object_spread(
    module: &mut Module,
    local_helper_context: &LocalHelperContext,
    module_facts: Option<&ModuleFactsMap>,
) {
    let mut local_helpers: HashMap<BindingKey, BabelHelperKind> = local_helper_context
        .helpers()
        .iter()
        .filter(|(_, kind)| {
            **kind == BabelHelperKind::Extends
                || **kind == BabelHelperKind::ObjectSpread
                || **kind == BabelHelperKind::DefineProperty
                || **kind == BabelHelperKind::HelperDependency
        })
        .map(|(key, kind)| (key.clone(), *kind))
        .collect();
    local_helpers.extend(collect_uninitialized_object_spread_stubs(module));
    let mut helpers = local_helpers.clone();
    if let Some(module_facts) = module_facts {
        helpers.extend(collect_cross_module_object_spread_helpers(
            module,
            module_facts,
        ));
    }
    let tslib_namespaces = local_helper_context.tslib_namespaces();
    if helpers.is_empty() && tslib_namespaces.is_empty() {
        return;
    }
    let mut replacer = SpreadReplacer {
        helpers: &helpers,
        tslib_namespaces,
    };
    module.visit_mut_with(&mut replacer);

    // Only remove root helpers whose calls were fully transformed. Dependencies
    // referenced by retained helpers must stay with those helpers.
    let local_root_helpers: HashMap<BindingKey, BabelHelperKind> = local_helpers
        .iter()
        .filter(|(_, kind)| {
            matches!(
                kind,
                BabelHelperKind::Extends | BabelHelperKind::ObjectSpread
            )
        })
        .map(|(key, kind)| (key.clone(), *kind))
        .collect();
    let remaining_roots = helpers_with_remaining_refs(module, &local_root_helpers);
    let removable_roots: HashMap<BindingKey, BabelHelperKind> = local_root_helpers
        .into_iter()
        .filter(|(key, _)| !remaining_roots.contains(key))
        .collect();
    let helper_dependencies = collect_helper_dependencies(module, &removable_roots);
    let standalone_dependencies = local_helpers.into_iter().filter(|(_, kind)| {
        matches!(
            kind,
            BabelHelperKind::DefineProperty | BabelHelperKind::HelperDependency
        )
    });
    let removable_helpers: HashMap<BindingKey, BabelHelperKind> = removable_roots
        .into_iter()
        .chain(helper_dependencies)
        .chain(standalone_dependencies)
        .collect();
    let remaining = helpers_with_remaining_refs(module, &removable_helpers);
    let safe_to_remove: HashMap<BindingKey, BabelHelperKind> = removable_helpers
        .into_iter()
        .filter(|(key, _)| !remaining.contains(key))
        .collect();
    if !safe_to_remove.is_empty() {
        remove_helper_declarations(&mut module.body, &safe_to_remove);
    }
}

impl Default for UnObjectSpread<'_> {
    fn default() -> Self {
        Self::new()
    }
}

fn collect_uninitialized_object_spread_stubs(
    module: &Module,
) -> HashMap<BindingKey, BabelHelperKind> {
    let mut helpers = HashMap::new();

    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            if decl.init.is_some() {
                continue;
            }
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            if matches!(binding.id.sym.as_ref(), "__spreadValues" | "__spreadProps") {
                helpers.insert(
                    (binding.id.sym.clone(), binding.id.ctxt),
                    BabelHelperKind::HelperDependency,
                );
            }
        }
    }

    helpers
}

fn collect_cross_module_object_spread_helpers(
    module: &Module,
    module_facts: &ModuleFactsMap,
) -> HashMap<BindingKey, BabelHelperKind> {
    let mut helpers = HashMap::new();

    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let source = str_to_atom(&import.src.value);

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Default(default) => {
                    if let Some(kind) = module_helper_export_kind(module_facts, &source, "default")
                    {
                        helpers.insert((default.local.sym.clone(), default.local.ctxt), kind);
                    }
                }
                ImportSpecifier::Named(named) => {
                    let imported = named
                        .imported
                        .as_ref()
                        .map(export_name_to_atom)
                        .unwrap_or_else(|| named.local.sym.clone());
                    if let Some(kind) =
                        module_helper_export_kind(module_facts, &source, imported.as_ref())
                    {
                        helpers.insert((named.local.sym.clone(), named.local.ctxt), kind);
                    }
                }
                ImportSpecifier::Namespace(_) => {}
            }
        }
    }

    helpers
}

fn module_helper_export_kind(
    module_facts: &ModuleFactsMap,
    source: &Atom,
    exported: &str,
) -> Option<BabelHelperKind> {
    module_facts.get(source.as_ref()).and_then(|facts| {
        facts
            .helper_exports
            .iter()
            .find(|helper| helper.exported.as_ref() == exported)
            .and_then(|helper| helper_kind_to_babel(helper.kind))
    })
}

fn helper_kind_to_babel(kind: HelperKind) -> Option<BabelHelperKind> {
    match kind {
        HelperKind::Extends => Some(BabelHelperKind::Extends),
        HelperKind::ObjectSpread => Some(BabelHelperKind::ObjectSpread),
        _ => None,
    }
}

struct SpreadReplacer<'a> {
    helpers: &'a HashMap<BindingKey, BabelHelperKind>,
    tslib_namespaces: &'a HashSet<BindingKey>,
}

impl VisitMut for SpreadReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        if !is_object_spread_callee(callee, self.helpers, self.tslib_namespaces) {
            return;
        }

        if call.args.is_empty() {
            return;
        }

        // Both _extends and _objectSpread2 mutate their first argument.
        // Only transform when the first arg is a safe fresh object literal
        // target, otherwise mutation/identity semantics are lost.
        let Expr::Object(first_obj) = call.args[0].expr.as_ref() else {
            return;
        };
        if call.args[0].spread.is_some() || !is_safe_to_inline_props(&first_obj.props) {
            return;
        }

        // Merge all arguments into a single object expression.
        // - Object literal args: flatten their properties
        // - Everything else: wrap as spread element
        let mut properties: Vec<PropOrSpread> = first_obj.props.clone();

        for arg in &call.args[1..] {
            if arg.spread.is_some() {
                properties.push(PropOrSpread::Spread(SpreadElement {
                    dot3_token: DUMMY_SP,
                    expr: arg.expr.clone(),
                }));
                continue;
            }

            match arg.expr.as_ref() {
                Expr::Object(obj) if is_safe_to_inline_props(&obj.props) => {
                    properties.extend(obj.props.iter().cloned());
                }
                _ => {
                    properties.push(PropOrSpread::Spread(SpreadElement {
                        dot3_token: DUMMY_SP,
                        expr: arg.expr.clone(),
                    }));
                }
            }
        }

        *expr = Expr::Object(ObjectLit {
            span: DUMMY_SP,
            props: properties,
        });
    }
}

fn is_object_spread_callee(
    callee: &Expr,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) -> bool {
    match strip_parens(callee) {
        Expr::Ident(id) => {
            let key = (id.sym.clone(), id.ctxt);
            matches!(
                helpers.get(&key),
                Some(BabelHelperKind::Extends | BabelHelperKind::ObjectSpread)
            )
        }
        Expr::Member(_) => matches!(
            tslib_member_helper_kind(callee, tslib_namespaces),
            Some(BabelHelperKind::Extends | BabelHelperKind::ObjectSpread)
        ),
        expr => is_inline_object_spread_helper(expr),
    }
}

fn is_inline_object_spread_helper(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => function_matches_inline_object_spread(&fn_expr.function),
        Expr::Arrow(arrow) => {
            let Some((target, source)) = arrow_param_pair(&arrow.params) else {
                return false;
            };
            match arrow.body.as_ref() {
                BlockStmtOrExpr::BlockStmt(block) => {
                    block_matches_inline_object_spread(&block.stmts, target, source)
                }
                BlockStmtOrExpr::Expr(expr) => {
                    expr_matches_inline_object_spread(expr, target, source)
                }
            }
        }
        _ => false,
    }
}

fn function_matches_inline_object_spread(function: &Function) -> bool {
    let Some(body) = &function.body else {
        return false;
    };
    let params: Vec<_> = function.params.iter().map(|param| &param.pat).collect();
    let Some((target, source)) = pat_pair(&params) else {
        return false;
    };
    block_matches_inline_object_spread(&body.stmts, target, source)
}

fn block_matches_inline_object_spread(stmts: &[Stmt], target: &Ident, source: &Ident) -> bool {
    if !matches!(
        stmts.last(),
        Some(Stmt::Return(ret)) if ret.arg.as_deref().is_some_and(|arg| is_binding_ref(arg, target))
    ) {
        return false;
    }

    let mut marker = InlineSpreadMarker::new(target, source);
    stmts.visit_with(&mut marker);
    marker.is_match()
}

fn expr_matches_inline_object_spread(expr: &Expr, target: &Ident, source: &Ident) -> bool {
    let mut marker = InlineSpreadMarker::new(target, source);
    expr.visit_with(&mut marker);
    marker.is_match()
}

fn arrow_param_pair(params: &[Pat]) -> Option<(&Ident, &Ident)> {
    let refs: Vec<_> = params.iter().collect();
    pat_pair(&refs)
}

fn pat_pair<'a>(params: &[&'a Pat]) -> Option<(&'a Ident, &'a Ident)> {
    let [Pat::Ident(target), Pat::Ident(source)] = params else {
        return None;
    };
    Some((&target.id, &source.id))
}

struct InlineSpreadMarker<'a> {
    target: &'a Ident,
    source: &'a Ident,
    saw_generated_spread_helper: bool,
    saw_target_helper_arg: bool,
    saw_source_ref: bool,
}

impl<'a> InlineSpreadMarker<'a> {
    fn new(target: &'a Ident, source: &'a Ident) -> Self {
        Self {
            target,
            source,
            saw_generated_spread_helper: false,
            saw_target_helper_arg: false,
            saw_source_ref: false,
        }
    }

    fn is_match(&self) -> bool {
        self.saw_generated_spread_helper && self.saw_target_helper_arg && self.saw_source_ref
    }
}

impl Visit for InlineSpreadMarker<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident_matches(ident, self.source) {
            self.saw_source_ref = true;
        }
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if is_generated_spread_helper_callee(&call.callee) {
            self.saw_generated_spread_helper = true;
            if call
                .args
                .first()
                .is_some_and(|arg| arg.spread.is_none() && is_binding_ref(&arg.expr, self.target))
            {
                self.saw_target_helper_arg = true;
            }
        }

        call.visit_children_with(self);
    }
}

fn is_generated_spread_helper_callee(callee: &Callee) -> bool {
    let Callee::Expr(expr) = callee else {
        return false;
    };
    match strip_parens(expr) {
        Expr::Ident(id) => matches!(id.sym.as_ref(), "__defNormalProp" | "__defProps"),
        Expr::Member(member) => match &member.prop {
            swc_core::ecma::ast::MemberProp::Ident(prop) => {
                matches!(prop.sym.as_ref(), "defineProperty" | "defineProperties")
            }
            swc_core::ecma::ast::MemberProp::PrivateName(_)
            | swc_core::ecma::ast::MemberProp::Computed(_) => false,
        },
        _ => false,
    }
}

fn is_binding_ref(expr: &Expr, binding: &Ident) -> bool {
    matches!(strip_parens(expr), Expr::Ident(id) if ident_matches(id, binding))
}

fn ident_matches(left: &Ident, right: &Ident) -> bool {
    left.sym == right.sym && left.ctxt == right.ctxt
}

fn is_safe_to_inline_props(props: &[PropOrSpread]) -> bool {
    props.iter().all(is_safe_to_inline_prop)
}

fn is_safe_to_inline_prop(prop: &PropOrSpread) -> bool {
    match prop {
        PropOrSpread::Spread(_) => true,
        PropOrSpread::Prop(prop) => match prop.as_ref() {
            Prop::Shorthand(ident) => ident.sym != "__proto__",
            Prop::KeyValue(kv) => !is_bare_proto_name(&kv.key),
            Prop::Assign(assign) => assign.key.sym != "__proto__",
            Prop::Getter(_) | Prop::Setter(_) | Prop::Method(_) => false,
        },
    }
}

fn is_bare_proto_name(name: &PropName) -> bool {
    match name {
        PropName::Ident(ident) => ident.sym == "__proto__",
        PropName::Str(value) => value.value == "__proto__",
        PropName::Num(_) | PropName::BigInt(_) | PropName::Computed(_) => false,
    }
}

fn export_name_to_atom(name: &swc_core::ecma::ast::ModuleExportName) -> Atom {
    match name {
        swc_core::ecma::ast::ModuleExportName::Ident(id) => id.sym.clone(),
        swc_core::ecma::ast::ModuleExportName::Str(s) => str_to_atom(&s.value),
    }
}

fn str_to_atom(value: &swc_core::atoms::Wtf8Atom) -> Atom {
    Atom::from(value.as_str().unwrap_or(""))
}
