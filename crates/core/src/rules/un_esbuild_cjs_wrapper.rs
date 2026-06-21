use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BlockStmtOrExpr, CallExpr, Callee, Decl, Expr, Function,
    Ident, Lit, MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, ObjectLit, Param, Pat,
    Prop, PropName, PropOrSpread, ReturnStmt, SimpleAssignTarget, Stmt, VarDeclarator,
};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMut, VisitWith};

use crate::utils::paren::strip_parens;

use super::decl_utils::{binding_id, same_ident, BindingId};

/// Unwraps a single esbuild CommonJS module helper after Terser inlines the
/// helper binding:
///
/// ```js
/// const require_stdin = ((cb, mod) => function require_stdin() { ... })({
///   "<stdin>"(exports) { body; }
/// });
/// export default require_stdin();
/// ```
///
/// The rule only lifts script-like factories whose `exports`/`module` params are
/// unused, so CommonJS export behavior is preserved by refusing non-script
/// modules.
pub struct UnEsbuildCjsWrapper;

impl UnEsbuildCjsWrapper {
    pub fn new(_: Mark) -> Self {
        Self
    }
}

impl VisitMut for UnEsbuildCjsWrapper {
    fn visit_mut_module(&mut self, module: &mut Module) {
        if let Some(body) = unwrap_single_module_wrapper(&module.body) {
            module.body = body;
        }
    }
}

fn unwrap_single_module_wrapper(body: &[ModuleItem]) -> Option<Vec<ModuleItem>> {
    if body.len() != 2 {
        return None;
    }

    let (wrapper, init) = wrapper_decl(&body[0])?;
    let exported = export_default_zero_arg_call(&body[1])?;
    if !same_ident(wrapper, exported) {
        return None;
    }

    let factory = inlined_commonjs_factory_body(init)?;
    if !factory_body_is_safe_to_lift(&factory.params, &factory.body) {
        return None;
    }

    Some(factory.body.into_iter().map(ModuleItem::Stmt).collect())
}

struct LiftedFactory {
    params: Vec<Param>,
    body: Vec<Stmt>,
}

fn wrapper_decl(item: &ModuleItem) -> Option<(&Ident, &Expr)> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let VarDeclarator {
        name: Pat::Ident(binding),
        init: Some(init),
        ..
    } = &var.decls[0]
    else {
        return None;
    };

    Some((&binding.id, init.as_ref()))
}

fn export_default_zero_arg_call(item: &ModuleItem) -> Option<&Ident> {
    let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) = item else {
        return None;
    };
    let Expr::Call(call) = strip_parens(export.expr.as_ref()) else {
        return None;
    };
    if !call.args.is_empty() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(ident) = strip_parens(callee.as_ref()) else {
        return None;
    };

    Some(ident)
}

fn inlined_commonjs_factory_body(init: &Expr) -> Option<LiftedFactory> {
    let Expr::Call(call) = strip_parens(init) else {
        return None;
    };
    if call.args.len() != 1 {
        return None;
    }

    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Arrow(helper) = strip_parens(callee.as_ref()) else {
        return None;
    };
    if helper.params.len() != 2 {
        return None;
    }
    let modules = pat_ident(&helper.params[0])?;
    let cache = pat_ident(&helper.params[1])?;

    let BlockStmtOrExpr::Expr(helper_body) = helper.body.as_ref() else {
        return None;
    };
    let Expr::Fn(require_fn) = strip_parens(helper_body.as_ref()) else {
        return None;
    };
    if !function_looks_like_commonjs_require(&require_fn.function, modules, cache) {
        return None;
    }

    let factory = single_object_method_function(call.args[0].expr.as_ref())?;
    Some(LiftedFactory {
        params: factory.params.clone(),
        body: factory.body.as_ref()?.stmts.clone(),
    })
}

fn pat_ident(pat: &Pat) -> Option<&Ident> {
    let Pat::Ident(binding) = pat else {
        return None;
    };
    Some(&binding.id)
}

fn function_looks_like_commonjs_require(
    function: &Function,
    modules: &Ident,
    cache: &Ident,
) -> bool {
    if !function.params.is_empty() {
        return false;
    }

    let Some(body) = &function.body else {
        return false;
    };
    let mut shape = CommonJsRequireShape {
        modules,
        cache,
        saw_module_dispatch: false,
        saw_cache_init: false,
        saw_cache_exports: false,
    };
    body.visit_with(&mut shape);

    shape.saw_module_dispatch && shape.saw_cache_init && shape.saw_cache_exports
}

struct CommonJsRequireShape<'a> {
    modules: &'a Ident,
    cache: &'a Ident,
    saw_module_dispatch: bool,
    saw_cache_init: bool,
    saw_cache_exports: bool,
}

impl Visit for CommonJsRequireShape<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if is_module_dispatch_call(call, self.modules) {
            self.saw_module_dispatch = true;
        }
        call.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if assigns_cache_exports_object(assign, self.cache) {
            self.saw_cache_init = true;
        }
        assign.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if is_cache_exports_member(member, self.cache) {
            self.saw_cache_exports = true;
        }
        member.visit_children_with(self);
    }
}

fn is_module_dispatch_call(call: &CallExpr, modules: &Ident) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = strip_parens(callee.as_ref()) else {
        return false;
    };
    matches!(member.obj.as_ref(), Expr::Ident(obj) if same_ident(obj, modules))
        && matches!(member.prop, MemberProp::Computed(_))
}

fn assigns_cache_exports_object(assign: &AssignExpr, cache: &Ident) -> bool {
    if assign.op != AssignOp::Assign {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assign.left else {
        return false;
    };
    matches!(strip_parens(assign.right.as_ref()), Expr::Object(obj) if same_ident(&target.id, cache) && object_has_only_empty_exports(obj))
}

fn object_has_only_empty_exports(obj: &ObjectLit) -> bool {
    let [PropOrSpread::Prop(prop)] = obj.props.as_slice() else {
        return false;
    };
    let Prop::KeyValue(key_value) = prop.as_ref() else {
        return false;
    };

    prop_name_is_exports(&key_value.key)
        && matches!(strip_parens(key_value.value.as_ref()), Expr::Object(inner) if inner.props.is_empty())
}

fn is_cache_exports_member(member: &MemberExpr, cache: &Ident) -> bool {
    matches!(member.obj.as_ref(), Expr::Ident(obj) if same_ident(obj, cache))
        && member_prop_is_exports(&member.prop)
}

fn member_prop_is_exports(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(ident) => ident.sym == *"exports",
        MemberProp::Computed(computed) => {
            matches!(strip_parens(computed.expr.as_ref()), Expr::Lit(Lit::Str(value)) if value.value == *"exports")
        }
        _ => false,
    }
}

fn prop_name_is_exports(name: &PropName) -> bool {
    match name {
        PropName::Ident(ident) => ident.sym == *"exports",
        PropName::Str(value) => value.value == *"exports",
        _ => false,
    }
}

fn single_object_method_function(expr: &Expr) -> Option<&Function> {
    let Expr::Object(obj) = strip_parens(expr) else {
        return None;
    };
    let [PropOrSpread::Prop(prop)] = obj.props.as_slice() else {
        return None;
    };
    let Prop::Method(method) = prop.as_ref() else {
        return None;
    };

    Some(&method.function)
}

fn factory_body_is_safe_to_lift(params: &[Param], body: &[Stmt]) -> bool {
    let param_bindings: Vec<BindingId> = params
        .iter()
        .flat_map(|param| find_pat_ids(&param.pat))
        .collect();

    let mut finder = UnsafeFactoryBodyRefFinder {
        param_bindings: &param_bindings,
        found: false,
        function_depth: 0,
    };
    for stmt in body {
        stmt.visit_with(&mut finder);
        if finder.found {
            return false;
        }
    }

    true
}

struct UnsafeFactoryBodyRefFinder<'a> {
    param_bindings: &'a [BindingId],
    found: bool,
    function_depth: usize,
}

impl Visit for UnsafeFactoryBodyRefFinder<'_> {
    fn visit_return_stmt(&mut self, _: &ReturnStmt) {
        if self.function_depth == 0 {
            self.found = true;
        }
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if self.param_bindings.contains(&binding_id(ident)) || ident.sym == *"arguments" {
            self.found = true;
        }
    }

    fn visit_binding_ident(&mut self, _: &swc_core::ecma::ast::BindingIdent) {}

    fn visit_this_expr(&mut self, _: &swc_core::ecma::ast::ThisExpr) {
        self.found = true;
    }

    fn visit_function(&mut self, function: &Function) {
        self.function_depth += 1;
        function.visit_children_with(self);
        self.function_depth -= 1;
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        self.function_depth += 1;
        arrow.visit_children_with(self);
        self.function_depth -= 1;
    }
}
