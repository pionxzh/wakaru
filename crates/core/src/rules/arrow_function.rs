use std::collections::HashSet;
use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};

use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, BinExpr, BinaryOp, BlockStmt, BlockStmtOrExpr, CallExpr, Callee, Class,
    Expr, FnExpr, Function, Ident, KeyValueProp, MemberExpr, MemberProp, MetaPropExpr,
    MetaPropKind, Module, NewExpr, Pat, ThisExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::constructor_sensitivity::{
    assign_target_value_key, collect_constructor_sensitive_values, is_bind_call, is_construct_call,
    pat_value_key, static_member_name, ValueKey,
};
use super::decl_utils::has_duplicate_param_names;
use super::eval_utils::{direct_eval_call_source, js_source_mentions_binding, EvalCallSource};

pub struct ArrowFunction;

impl VisitMut for ArrowFunction {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let constructor_sensitive_values = collect_constructor_sensitive_values(module);
        module.visit_mut_with(&mut ArrowFunctionConverter {
            constructor_sensitive_values: &constructor_sensitive_values,
        });
    }
}

struct ArrowFunctionConverter<'a> {
    constructor_sensitive_values: &'a HashSet<ValueKey>,
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
        let Some(init) = &mut decl.init else {
            return;
        };

        if pat_value_key(&decl.name)
            .is_some_and(|key| self.constructor_sensitive_values.contains(&key))
        {
            visit_constructor_value_without_converting(init, self);
            return;
        }

        init.visit_mut_with(self);
    }

    fn visit_mut_assign_expr(&mut self, expr: &mut AssignExpr) {
        expr.left.visit_mut_with(self);
        if assign_target_value_key(&expr.left)
            .is_some_and(|key| self.constructor_sensitive_values.contains(&key))
        {
            visit_constructor_value_without_converting(&mut expr.right, self);
            return;
        }
        expr.right.visit_mut_with(self);
    }

    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        call.callee.visit_mut_with(self);

        let construct_call = is_construct_call(call);
        for (index, arg) in call.args.iter_mut().enumerate() {
            if construct_call && (index == 0 || index == 2) {
                // Reflect.construct requires both target and newTarget to be
                // constructible. Preserve ordinary functions in either slot.
                visit_constructor_value_without_converting(&mut arg.expr, self);
            } else {
                arg.visit_mut_with(self);
            }
        }
    }

    fn visit_mut_new_expr(&mut self, expr: &mut NewExpr) {
        visit_constructor_value_without_converting(&mut expr.callee, self);
        expr.args.visit_mut_with(self);
        expr.type_args.visit_mut_with(self);
    }

    fn visit_mut_bin_expr(&mut self, expr: &mut BinExpr) {
        if expr.op == BinaryOp::InstanceOf {
            expr.left.visit_mut_with(self);
            visit_constructor_value_without_converting(&mut expr.right, self);
        } else {
            expr.visit_mut_children_with(self);
        }
    }

    fn visit_mut_class(&mut self, class: &mut Class) {
        let mut super_class = class.super_class.take();
        class.visit_mut_children_with(self);
        if let Some(super_class) = &mut super_class {
            visit_constructor_value_without_converting(super_class, self);
        }
        class.super_class = super_class;
    }

    fn visit_mut_member_expr(&mut self, member: &mut MemberExpr) {
        if static_member_name(&member.prop).is_some_and(|name| name == "prototype") {
            visit_constructor_value_without_converting(&mut member.obj, self);
            if let MemberProp::Computed(computed) = &mut member.prop {
                computed.expr.visit_mut_with(self);
            }
        } else {
            member.visit_mut_children_with(self);
        }
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

fn visit_constructor_value_without_converting(
    expr: &mut Expr,
    converter: &mut ArrowFunctionConverter<'_>,
) {
    match expr {
        Expr::Fn(fn_expr) => {
            if let Some(body) = &mut fn_expr.function.body {
                body.visit_mut_with(converter);
            }
        }
        Expr::Paren(paren) => {
            visit_constructor_value_without_converting(&mut paren.expr, converter);
        }
        Expr::Seq(sequence) => {
            if let Some((last, prefix)) = sequence.exprs.split_last_mut() {
                for expr in prefix {
                    expr.visit_mut_with(converter);
                }
                visit_constructor_value_without_converting(last, converter);
            }
        }
        Expr::Cond(conditional) => {
            conditional.test.visit_mut_with(converter);
            visit_constructor_value_without_converting(&mut conditional.cons, converter);
            visit_constructor_value_without_converting(&mut conditional.alt, converter);
        }
        Expr::Bin(binary)
            if matches!(
                binary.op,
                BinaryOp::LogicalOr | BinaryOp::LogicalAnd | BinaryOp::NullishCoalescing
            ) =>
        {
            visit_constructor_value_without_converting(&mut binary.left, converter);
            visit_constructor_value_without_converting(&mut binary.right, converter);
        }
        Expr::Call(call) if is_bind_call(call) => {
            let Callee::Expr(callee) = &mut call.callee else {
                unreachable!();
            };
            let Expr::Member(member) = callee.as_mut() else {
                unreachable!();
            };
            visit_constructor_value_without_converting(&mut member.obj, converter);
            call.args.visit_mut_with(converter);
            call.type_args.visit_mut_with(converter);
        }
        _ => expr.visit_mut_with(converter),
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

    // Arrow parameter lists reject duplicate names as an early error; a
    // sloppy-mode function may carry them.
    if has_duplicate_param_names(&func.params) {
        return None;
    }

    // Must have a body
    let body = func.body.as_ref()?;

    // Direct eval can observe function-only bindings (`this`, `arguments`,
    // `new.target`) that are not visible to an AST walk of the containing
    // function. Keep the function shape rather than guessing from source text.
    if body_has_arrow_sensitive_direct_eval(body, true) {
        return None;
    }

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
        type_params: fn_expr.function.type_params.take(),
        return_type: fn_expr.function.return_type.take(),
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

    // Arrow parameter lists reject duplicate names as an early error.
    if has_duplicate_param_names(&func.params) {
        return None;
    }

    if func
        .body
        .as_ref()
        .is_some_and(|body| body_has_arrow_sensitive_direct_eval(body, false))
    {
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

fn body_has_arrow_sensitive_direct_eval(body: &BlockStmt, include_this: bool) -> bool {
    let mut analyzer = ArrowSensitiveDirectEvalAnalyzer::default();
    body.visit_with(&mut analyzer);
    if analyzer.unknown_direct_eval {
        return true;
    }

    // `new` is deliberately broader than the exact `new.target` spelling so
    // whitespace/comments in evaluated source cannot evade the guard.
    let arguments_name: Atom = "arguments".into();
    let new_name: Atom = "new".into();
    let this_name: Atom = "this".into();
    analyzer.known_direct_eval_sources.iter().any(|source| {
        js_source_mentions_binding(source, &arguments_name)
            || js_source_mentions_binding(source, &new_name)
            || (include_this && js_source_mentions_binding(source, &this_name))
    })
}

#[derive(Default)]
struct ArrowSensitiveDirectEvalAnalyzer {
    known_direct_eval_sources: Vec<String>,
    unknown_direct_eval: bool,
}

impl Visit for ArrowSensitiveDirectEvalAnalyzer {
    fn visit_call_expr(&mut self, expr: &CallExpr) {
        if let Some(source) = direct_eval_call_source(expr) {
            match source {
                EvalCallSource::NoSource => {}
                EvalCallSource::Known(source) => self.known_direct_eval_sources.push(source),
                EvalCallSource::Unknown => self.unknown_direct_eval = true,
            }
            for arg in &expr.args {
                arg.expr.visit_with(self);
            }
            return;
        }

        expr.visit_children_with(self);
    }

    // Regular functions have their own `this`, `arguments`, and `new.target`.
    // Arrows still recurse through the default visitor because they capture
    // those bindings from the function being considered for conversion.
    fn visit_function(&mut self, _: &Function) {}
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

    fn visit_meta_prop_expr(&mut self, expr: &MetaPropExpr) {
        if expr.kind == MetaPropKind::NewTarget {
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
