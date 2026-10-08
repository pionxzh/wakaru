use crate::analysis::binding_uses::BindingId;
use crate::collections::{HashMap, HashSet};
use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};

use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, AssignExpr, AssignTarget, BinExpr, BinaryOp, CallExpr, Callee,
    Class, DefaultDecl, ExportDefaultDecl, Expr, FnDecl, FnExpr, Function, FunctionBody, Ident,
    KeyValueProp, MemberExpr, MemberProp, MetaPropExpr, MetaPropKind, Module, NewExpr, Pat,
    ReturnStmt, ThisExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::constructor_sensitivity::{
    assign_target_value_key, collect_constructor_sensitive_values, is_bind_call, is_construct_call,
    is_sync_iife_call, pat_value_key, static_member_name,
    visit_mut_assign_target_pat_constructor_sensitive_defaults,
    visit_mut_pat_constructor_sensitive_defaults, CreateClassHelpers, ValueKey,
};
use super::decl_utils::has_duplicate_param_names;
use super::eval_utils::{direct_eval_call_source, js_source_mentions_binding, EvalCallSource};
use super::transpiler_helper_utils::LocalHelperContext;

pub struct ArrowFunction {
    unresolved_mark: Mark,
}

impl ArrowFunction {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }

    pub(crate) fn run_with_helpers(
        module: &mut Module,
        unresolved_mark: Mark,
        local_helpers: &LocalHelperContext,
    ) {
        let create_class = CreateClassHelpers::collect(module, unresolved_mark, local_helpers);
        let constructor_sensitive_values =
            collect_constructor_sensitive_values(module, &create_class);
        // Collected before rewriting. Function declarations are hoisted, so a
        // call may appear above the declaration.
        let declared_parameters =
            collect_declared_function_parameters(module, &constructor_sensitive_values);
        module.visit_mut_with(&mut ArrowFunctionConverter {
            constructor_sensitive_values: &constructor_sensitive_values,
            declared_parameters: &declared_parameters,
            create_class: &create_class,
            protect_iife_callee: false,
            protect_next_body_returns: false,
            protect_returns: false,
        });
    }
}

impl VisitMut for ArrowFunction {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect_with_mark(module, self.unresolved_mark);
        Self::run_with_helpers(module, self.unresolved_mark, &local_helpers);
    }
}

struct ArrowFunctionConverter<'a> {
    constructor_sensitive_values: &'a HashSet<ValueKey>,
    /// Parameter sensitivity for same-module `FnDecl`s and named
    /// `export default function`s. Keyed by the function name's `(sym, ctxt)`.
    declared_parameters: &'a HashMap<BindingId, Vec<bool>>,
    create_class: &'a CreateClassHelpers,
    /// The next call visited is a constructor-sensitive IIFE: its callee's own
    /// `return` values are the result and must stay constructible.
    protect_iife_callee: bool,
    /// The next function body entered is that IIFE callee's body.
    protect_next_body_returns: bool,
    /// Returns in the current function body are the protected IIFE's result.
    protect_returns: bool,
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
        let constructor_sensitive_values = self.constructor_sensitive_values;
        visit_mut_pat_constructor_sensitive_defaults(
            &mut decl.name,
            constructor_sensitive_values,
            &mut |expr, is_constructor_sensitive| {
                if is_constructor_sensitive {
                    visit_constructor_value_without_converting(expr, self);
                } else {
                    expr.visit_mut_with(self);
                }
            },
        );
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
        match &mut expr.left {
            AssignTarget::Simple(target) => target.visit_mut_with(self),
            AssignTarget::Pat(pat) => {
                let constructor_sensitive_values = self.constructor_sensitive_values;
                visit_mut_assign_target_pat_constructor_sensitive_defaults(
                    pat,
                    constructor_sensitive_values,
                    &mut |expr, is_constructor_sensitive| {
                        if is_constructor_sensitive {
                            visit_constructor_value_without_converting(expr, self);
                        } else {
                            expr.visit_mut_with(self);
                        }
                    },
                );
            }
        }
        if assign_target_value_key(&expr.left)
            .is_some_and(|key| self.constructor_sensitive_values.contains(&key))
        {
            visit_constructor_value_without_converting(&mut expr.right, self);
            return;
        }
        expr.right.visit_mut_with(self);
    }

    fn visit_mut_function_body(&mut self, body: &mut FunctionBody) {
        // Each body owns its returns: only the protected IIFE callee's body
        // takes the flag, and a nested function body starts unprotected.
        let saved = self.protect_returns;
        self.protect_returns = std::mem::take(&mut self.protect_next_body_returns);
        body.visit_mut_children_with(self);
        self.protect_returns = saved;
    }

    fn visit_mut_return_stmt(&mut self, stmt: &mut ReturnStmt) {
        match &mut stmt.arg {
            Some(arg) if self.protect_returns => {
                visit_constructor_value_without_converting(arg, self);
            }
            _ => stmt.visit_mut_children_with(self),
        }
    }

    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        // Read the parameter list before rewriting the callee. A literal callee
        // may itself become an arrow; pairing uses the pre-rewrite parameters.
        let pairing = call_parameter_pairing(call, self);
        if std::mem::take(&mut self.protect_iife_callee) {
            if let Callee::Expr(callee) = &mut call.callee {
                visit_iife_callee_protecting_returns(callee, self);
            }
        } else {
            call.callee.visit_mut_with(self);
        }

        let construct_call = is_construct_call(call);
        let create_class_call = self.create_class.is_call(call);
        // Pairing stops at the first spread: its runtime length makes later
        // syntactic positions unknown.
        let spread_at = call.args.iter().position(|arg| arg.spread.is_some());
        for (index, arg) in call.args.iter_mut().enumerate() {
            let before_spread = spread_at.is_none_or(|at| index < at);
            let paired = before_spread
                && pairing.as_ref().is_some_and(|(offset, flags)| {
                    index >= *offset && flags.get(index - *offset) == Some(&true)
                });
            if (construct_call && (index == 0 || index == 2))
                || (create_class_call && index == 0)
                || paired
            {
                // Reflect.construct requires both target and newTarget to be
                // constructible. createClass defines methods on its first
                // argument's prototype, and callers construct the result. A
                // known constructor parameter of a literal callee, or of a
                // same-module function declaration, needs the same preservation.
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
        visit_constructor_value_without_converting(&mut export.expr, self);
    }
}

/// Shared argument pairing for a literal callee and a same-module function
/// declaration. Returns `(argument offset, per-parameter sensitivity)`.
/// `.call`'s first argument is `this`, so the offset is 1. Aliases and
/// `.apply` are not positional calls.
fn call_parameter_pairing(
    call: &CallExpr,
    converter: &ArrowFunctionConverter<'_>,
) -> Option<(usize, Vec<bool>)> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let target = peel_paren_and_sequence(callee);
    if let Expr::Member(member) = target {
        if static_member_name(&member.prop).as_deref() == Some("call") {
            let flags = parameter_flags(&member.obj, converter)?;
            return Some((1, flags));
        }
        return None;
    }
    parameter_flags(target, converter).map(|flags| (0, flags))
}

fn parameter_flags(expr: &Expr, converter: &ArrowFunctionConverter<'_>) -> Option<Vec<bool>> {
    match peel_paren_and_sequence(expr) {
        Expr::Fn(function) => Some(sensitive_param_flags(
            function.function.params.iter().map(|param| &param.pat),
            converter.constructor_sensitive_values,
        )),
        Expr::Arrow(arrow) => Some(sensitive_param_flags(
            arrow.params.iter(),
            converter.constructor_sensitive_values,
        )),
        Expr::Ident(ident) => converter
            .declared_parameters
            .get(&(ident.sym.clone(), ident.ctxt))
            .cloned(),
        _ => None,
    }
}

fn sensitive_param_flags<'a>(
    pats: impl Iterator<Item = &'a Pat>,
    sensitive: &HashSet<ValueKey>,
) -> Vec<bool> {
    pats.map(|pat| pat_value_key(pat).is_some_and(|key| sensitive.contains(&key)))
        .collect()
}

/// Parentheses and a sequence's last expression are the invoked value.
/// A conditional callee is left alone.
fn peel_paren_and_sequence(expr: &Expr) -> &Expr {
    match crate::utils::paren::strip_parens(expr) {
        Expr::Seq(sequence) => match sequence.exprs.last() {
            Some(last) => peel_paren_and_sequence(last),
            None => crate::utils::paren::strip_parens(expr),
        },
        other => other,
    }
}

fn collect_declared_function_parameters(
    module: &Module,
    sensitive: &HashSet<ValueKey>,
) -> HashMap<BindingId, Vec<bool>> {
    let mut collector = DeclaredFunctionParams {
        sensitive,
        declared: HashMap::default(),
    };
    module.visit_with(&mut collector);
    collector.declared
}

struct DeclaredFunctionParams<'a> {
    sensitive: &'a HashSet<ValueKey>,
    declared: HashMap<BindingId, Vec<bool>>,
}

impl DeclaredFunctionParams<'_> {
    fn record(&mut self, ident: &Ident, function: &Function) {
        let flags = sensitive_param_flags(
            function.params.iter().map(|param| &param.pat),
            self.sensitive,
        );
        let key = (ident.sym.clone(), ident.ctxt);
        match self.declared.get_mut(&key) {
            Some(existing) => or_param_flags(existing, &flags),
            None => {
                self.declared.insert(key, flags);
            }
        }
    }
}

/// When one binding has more than one declaration, OR the flags so a later
/// declaration cannot clear a slot that was already sensitive.
fn or_param_flags(existing: &mut Vec<bool>, flags: &[bool]) {
    if flags.len() > existing.len() {
        existing.resize(flags.len(), false);
    }
    for (index, flag) in flags.iter().enumerate() {
        if *flag {
            existing[index] = true;
        }
    }
}

impl Visit for DeclaredFunctionParams<'_> {
    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.record(&decl.ident, &decl.function);
        decl.visit_children_with(self);
    }

    fn visit_export_default_decl(&mut self, decl: &ExportDefaultDecl) {
        if let DefaultDecl::Fn(func) = &decl.decl {
            if let Some(ident) = &func.ident {
                self.record(ident, &func.function);
            }
        }
        decl.visit_children_with(self);
    }
}

fn visit_constructor_value_without_converting(
    expr: &mut Expr,
    converter: &mut ArrowFunctionConverter<'_>,
) {
    match expr {
        Expr::Fn(fn_expr) if fn_expr.function.is_async => {
            fn_expr.visit_mut_children_with(converter);
            if let Some(arrow) = try_convert_to_arrow(fn_expr) {
                *expr = Expr::Arrow(arrow);
            }
        }
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
        // An IIFE evaluates to what its callee returns, the same shapes
        // `constructor_sensitivity` follows for returned bindings. A
        // directly returned function has no binding to mark, so protect the
        // callee's return positions here.
        Expr::Call(call) if is_sync_iife_call(call) => {
            converter.protect_iife_callee = true;
            expr.visit_mut_with(converter);
        }
        _ => expr.visit_mut_with(converter),
    }
}

/// Mirrors `iife_callee_function`: parens, a sequence's last expression, and
/// the receiver of `.call` / `.apply` lead to the invoked function. The
/// callee itself may still become an arrow; only its returns are protected.
fn visit_iife_callee_protecting_returns(
    callee: &mut Expr,
    converter: &mut ArrowFunctionConverter<'_>,
) {
    match callee {
        Expr::Paren(paren) => visit_iife_callee_protecting_returns(&mut paren.expr, converter),
        Expr::Seq(sequence) => {
            if let Some((last, prefix)) = sequence.exprs.split_last_mut() {
                for expr in prefix {
                    expr.visit_mut_with(converter);
                }
                visit_iife_callee_protecting_returns(last, converter);
            }
        }
        Expr::Member(member)
            if static_member_name(&member.prop)
                .is_some_and(|name| name == "call" || name == "apply") =>
        {
            visit_iife_callee_protecting_returns(&mut member.obj, converter);
            if let MemberProp::Computed(computed) = &mut member.prop {
                computed.expr.visit_mut_with(converter);
            }
        }
        Expr::Fn(fn_expr) if !fn_expr.function.is_async && !fn_expr.function.is_generator => {
            // Parameter defaults can hold function bodies of their own, so
            // arm the flag only for this function's body.
            fn_expr.function.params.visit_mut_with(converter);
            fn_expr.function.decorators.visit_mut_with(converter);
            if let Some(body) = &mut fn_expr.function.body {
                converter.protect_next_body_returns = true;
                body.visit_mut_with(converter);
            }
            if let Some(arrow) = try_convert_to_arrow(fn_expr) {
                *callee = Expr::Arrow(arrow);
            }
        }
        Expr::Arrow(arrow) if !arrow.is_async && !arrow.is_generator => {
            arrow.params.visit_mut_with(converter);
            match arrow.body.as_mut() {
                ArrowFunctionBody::Expr(expr) => {
                    visit_constructor_value_without_converting(expr, converter);
                }
                ArrowFunctionBody::FunctionBody(body) => {
                    converter.protect_next_body_returns = true;
                    body.visit_mut_with(converter);
                }
            }
        }
        callee => callee.visit_mut_with(converter),
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
    func.body.as_ref()?;

    // Direct eval can observe function-only bindings (`this`, `arguments`,
    // `new.target`) that are not visible to an AST walk of the containing
    // function. Keep the function shape rather than guessing from source text.
    if function_has_arrow_sensitive_direct_eval(func, true) {
        return None;
    }

    // Check for this or arguments usage (don't recurse into nested functions).
    // Parameter initializers run inside the function's own scope, so they
    // bind `this`, `arguments`, and `new.target` exactly like the body does.
    let mut checker = HasThisOrArguments(false);
    visit_params_and_body(func, &mut checker);
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
fn build_arrow_body(func: &Function) -> ArrowFunctionBody {
    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return ArrowFunctionBody::FunctionBody(Default::default()),
    };

    ArrowFunctionBody::FunctionBody(body.clone())
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

    // `function () {}.bind(this)` and the arrow capture the same `this`, so a
    // known eval source that mentions only `this` is safe here; `arguments`
    // and `new.target` still block the conversion.
    if function_has_arrow_sensitive_direct_eval(func, false) {
        return None;
    }

    // Reject functions that use `arguments` (arrows have no own `arguments`),
    // in parameter initializers as well as in the body.
    let mut has_args = HasArguments(false);
    visit_params_and_body(func, &mut has_args);
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

/// Runs `visitor` over the parameter list and the body of `func`: both are
/// evaluated in the function's own activation, so a check for function-only
/// bindings must cover parameter initializers and patterns too.
fn visit_params_and_body<V: Visit>(func: &Function, visitor: &mut V) {
    for param in &func.params {
        param.visit_with(visitor);
    }
    if let Some(body) = &func.body {
        body.visit_with(visitor);
    }
}

fn function_has_arrow_sensitive_direct_eval(func: &Function, include_this: bool) -> bool {
    let mut analyzer = ArrowSensitiveDirectEvalAnalyzer::default();
    visit_params_and_body(func, &mut analyzer);
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

pub(crate) struct HasThisOrArguments(bool);

/// Whether `node` reads `this`, `arguments`, or `new.target` of the function
/// around it, outside nested non-arrow functions.
pub(crate) fn has_this_or_arguments<T>(node: &T) -> bool
where
    T: VisitWith<HasThisOrArguments> + ?Sized,
{
    let mut checker = HasThisOrArguments(false);
    node.visit_with(&mut checker);
    checker.0
}

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
