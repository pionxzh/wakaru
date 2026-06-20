use std::collections::HashSet;
use swc_core::common::{SyntaxContext, DUMMY_SP};

use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, BlockStmtOrExpr, CallExpr, Callee, Expr, FnExpr, Function,
    Ident, KeyValueProp, MemberProp, Module, Pat, SimpleAssignTarget, ThisExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::{BindingId, BindingUseIndex};

pub struct ArrowFunction;

impl VisitMut for ArrowFunction {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let constructed_bindings = collect_constructed_bindings(module);
        module.visit_mut_with(&mut ArrowFunctionConverter {
            constructed_bindings: &constructed_bindings,
        });
    }
}

struct ArrowFunctionConverter<'a> {
    constructed_bindings: &'a HashSet<BindingId>,
}

impl VisitMut for ArrowFunctionConverter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Fn(fn_expr) = expr {
            if let Some(arrow) = try_convert_to_arrow(fn_expr) {
                *expr = Expr::Arrow(arrow);
            }
            return;
        }

        // Handle `function(...) { ... }.bind(this)` → arrow function
        if let Expr::Call(call_expr) = expr {
            if let Some(arrow) = try_convert_bind_this(call_expr) {
                *expr = Expr::Arrow(arrow);
            }
        }
    }

    fn visit_mut_var_declarator(&mut self, decl: &mut VarDeclarator) {
        decl.name.visit_mut_with(self);
        let is_constructed_binding =
            declarator_binds_constructed_name(decl, self.constructed_bindings);
        let Some(init) = &mut decl.init else {
            return;
        };

        if is_constructed_binding {
            visit_fn_body_without_converting(init, self);
            return;
        }

        init.visit_mut_with(self);
    }

    fn visit_mut_assign_expr(&mut self, expr: &mut AssignExpr) {
        expr.left.visit_mut_with(self);
        if assign_target_is_constructed_name(&expr.left, self.constructed_bindings) {
            visit_fn_body_without_converting(&mut expr.right, self);
            return;
        }
        expr.right.visit_mut_with(self);
    }

    fn visit_mut_key_value_prop(&mut self, prop: &mut KeyValueProp) {
        // Object property function values are handled by ObjMethodShorthand.
        // ArrowFunction must not convert them to arrows — that would produce
        // `{"foo": () => {}}` which is not method syntax.
        // We still recurse into the function body so inner expressions are processed.
        prop.key.visit_mut_with(self);
        if let Expr::Fn(fn_expr) = prop.value.as_mut() {
            if let Some(body) = &mut fn_expr.function.body {
                body.visit_mut_with(self);
            }
        } else {
            prop.value.visit_mut_with(self);
        }
    }

    fn visit_mut_export_default_expr(
        &mut self,
        export: &mut swc_core::ecma::ast::ExportDefaultExpr,
    ) {
        // A default-exported function expression remains constructable by
        // consumers. Converting it to an arrow would remove its prototype.
        if let Expr::Fn(fn_expr) = export.expr.as_mut() {
            if let Some(body) = &mut fn_expr.function.body {
                body.visit_mut_with(self);
            }
        } else {
            export.expr.visit_mut_with(self);
        }
    }
}

fn collect_constructed_bindings(module: &Module) -> HashSet<BindingId> {
    BindingUseIndex::collect(module).new_callee_bindings()
}

fn declarator_binds_constructed_name(
    decl: &VarDeclarator,
    constructed_bindings: &HashSet<BindingId>,
) -> bool {
    let Pat::Ident(binding) = &decl.name else {
        return false;
    };
    constructed_bindings.contains(&(binding.id.sym.clone(), binding.id.ctxt))
}

fn assign_target_is_constructed_name(
    target: &AssignTarget,
    constructed_bindings: &HashSet<BindingId>,
) -> bool {
    let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = target else {
        return false;
    };
    constructed_bindings.contains(&(binding.id.sym.clone(), binding.id.ctxt))
}

fn visit_fn_body_without_converting(expr: &mut Expr, converter: &mut ArrowFunctionConverter<'_>) {
    if let Expr::Fn(fn_expr) = expr {
        if let Some(body) = &mut fn_expr.function.body {
            body.visit_mut_with(converter);
        }
    } else {
        expr.visit_mut_with(converter);
    }
}

fn try_convert_to_arrow(fn_expr: &mut FnExpr) -> Option<ArrowExpr> {
    let func = &fn_expr.function;

    // Don't convert generators
    if func.is_generator {
        return None;
    }

    // Named function expressions expose the name through `.name`. Converting
    // them to arrows can erase or change that observable value.
    if fn_expr.ident.is_some() {
        return None;
    }

    // Must have a body
    let body = func.body.as_ref()?;

    // Check for this or arguments usage (don't recurse into nested functions)
    let mut checker = HasThisOrArguments(false);
    body.visit_with(&mut checker);
    if checker.0 {
        return None;
    }

    // Convert params: Vec<Param> -> Vec<Pat>
    let params: Vec<Pat> = fn_expr
        .function
        .params
        .iter()
        .map(|p| p.pat.clone())
        .collect();

    // Build the arrow body
    let arrow_body = build_arrow_body(&fn_expr.function);

    Some(ArrowExpr {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        params,
        body: Box::new(arrow_body),
        is_async: fn_expr.function.is_async,
        is_generator: false,
        type_params: fn_expr.function.type_params.clone(),
        return_type: fn_expr.function.return_type.clone(),
    })
}

/// Build the arrow body:
/// - Always keep the original block body.
/// - ArrowReturn is responsible for `{ return expr; }` → `expr`.
fn build_arrow_body(func: &Function) -> BlockStmtOrExpr {
    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return BlockStmtOrExpr::BlockStmt(Default::default()),
    };

    BlockStmtOrExpr::BlockStmt(body.clone())
}

/// Try to convert `fn.bind(this)` to an arrow function.
/// Only fires when args is exactly `[this]` (no partial application).
/// The function may use `this` — that's the whole point of `.bind(this)`.
/// Still rejects: named functions, generators, functions using `arguments`.
fn try_convert_bind_this(call: &CallExpr) -> Option<ArrowExpr> {
    // Callee must be `expr.bind`
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee_expr.as_ref() else {
        return None;
    };
    let MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    if prop.sym != "bind" {
        return None;
    }

    // Must have exactly one argument and it must be `this` (no partial application)
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    if !matches!(call.args[0].expr.as_ref(), Expr::This(_)) {
        return None;
    }

    // The bound expression must be a function expression
    let Expr::Fn(fn_expr) = member.obj.as_ref() else {
        return None;
    };
    let func = &fn_expr.function;

    // Reject generators and named function expressions
    if func.is_generator || fn_expr.ident.is_some() {
        return None;
    }

    // Reject functions that use `arguments` (arrows have no own `arguments`)
    let mut has_args = HasArguments(false);
    if let Some(body) = &func.body {
        body.visit_with(&mut has_args);
    }
    if has_args.0 {
        return None;
    }

    let params: Vec<Pat> = func.params.iter().map(|p| p.pat.clone()).collect();
    let arrow_body = build_arrow_body(func);

    Some(ArrowExpr {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        params,
        body: Box::new(arrow_body),
        is_async: func.is_async,
        is_generator: false,
        type_params: func.type_params.clone(),
        return_type: func.return_type.clone(),
    })
}

// ============================================================
// Visitor: check for `this` or `arguments` (not in nested fns)
// ============================================================

struct HasThisOrArguments(bool);

impl Visit for HasThisOrArguments {
    fn visit_this_expr(&mut self, _: &ThisExpr) {
        self.0 = true;
    }

    fn visit_ident(&mut self, id: &Ident) {
        if id.sym == "arguments" {
            self.0 = true;
        }
    }

    // Don't recurse into nested functions — they have their own this/arguments
    fn visit_function(&mut self, _: &Function) {}

    // Recurse into arrow expressions because they capture both `this` and
    // `arguments` from this function.
}

// ============================================================
// Visitor: check for `arguments` only (not `this`)
// ============================================================

struct HasArguments(bool);

impl Visit for HasArguments {
    fn visit_ident(&mut self, id: &Ident) {
        if id.sym == "arguments" {
            self.0 = true;
        }
    }

    fn visit_function(&mut self, _: &Function) {}
}
