use crate::analysis::binding_uses::{BindingId, BindingUseIndex, UseKind};
use crate::collections::{HashMap, HashSet};
use crate::utils::member::static_member_name;
use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};

use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, BinaryOp, CallExpr, Callee, Decl, Expr, FnDecl, FnExpr, Function,
    Ident, KeyValueProp, MemberProp, MetaPropExpr, MetaPropKind, Module, ModuleDecl, ModuleItem,
    NewExpr, Pat, ThisExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::decl_utils::has_duplicate_param_names;
use super::eval_utils::{
    direct_eval_call_source, has_dynamic_scope_construct, js_source_mentions_binding,
    EvalCallSource,
};

/// Rewrites `function` expressions to arrows where positive evidence shows the
/// value is never constructed. An arrow has no `[[Construct]]` and no
/// `prototype`, and "never constructed" cannot be decided for an arbitrary
/// value, so a function in any other position stays a function. The evidence:
///
/// - an immediately invoked callee;
/// - a binding declared once, never written, not exported, and only called
///   (`f()`, `f.call()`, `f.apply()`, `typeof f`); a binding declared at the
///   top level of a script is a shared global and does not qualify;
/// - an inline argument whose same-module callee only calls the matching
///   simple parameter;
/// - an async function, which is never constructible;
/// - `builtin_callbacks_not_constructed`: an inline callback of a timer
///   global, `new Promise`, or a built-in array/Promise/string method name.
pub struct ArrowFunction {
    unresolved_mark: Mark,
}

impl ArrowFunction {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for ArrowFunction {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let evidence = Evidence::collect(module, self.unresolved_mark);
        module.visit_mut_with(&mut ArrowFunctionConverter {
            evidence: &evidence,
        });
    }
}

/// Global functions whose first argument is a callback the host only calls.
const CALLBACK_GLOBALS: &[&str] = &[
    "setTimeout",
    "setInterval",
    "setImmediate",
    "queueMicrotask",
    "requestAnimationFrame",
    "requestIdleCallback",
];

/// Argument positions of the callback a built-in method only calls. The list
/// is closed: a name joins it with evidence that the callbacks it receives
/// are lowered functions, not with a guess about a library API.
fn builtin_callback_positions(method: &str) -> &'static [usize] {
    match method {
        "map" | "forEach" | "filter" | "some" | "every" | "find" | "findIndex" | "findLast"
        | "findLastIndex" | "flatMap" | "sort" | "reduce" | "reduceRight" | "catch" | "finally" => {
            &[0]
        }
        "then" => &[0, 1],
        "replace" | "replaceAll" => &[1],
        _ => &[],
    }
}

struct Evidence {
    unresolved_mark: Mark,
    uses: BindingUseIndex,
    /// `with` or direct `eval` can reach any binding by name.
    dynamic_scope: bool,
    /// The module has import/export syntax, so its top-level bindings are
    /// module-scoped rather than shared script globals.
    is_es_module: bool,
    exported_declarations: HashSet<BindingId>,
    /// Simple parameters of same-module functions, by the function's binding;
    /// `None` for a parameter that is not a plain identifier. Functions that
    /// read their own `arguments` are left out.
    function_params: HashMap<BindingId, Vec<Option<BindingId>>>,
}

impl Evidence {
    fn collect(module: &Module, unresolved_mark: Mark) -> Self {
        let mut params = FunctionParamCollector::default();
        module.visit_with(&mut params);
        Self {
            unresolved_mark,
            uses: BindingUseIndex::collect(module),
            dynamic_scope: has_dynamic_scope_construct(module),
            is_es_module: module
                .body
                .iter()
                .any(|item| matches!(item, ModuleItem::ModuleDecl(_))),
            exported_declarations: exported_declarations(module),
            function_params: params.params,
        }
    }

    /// Whether every use of the binding calls it, so its value cannot reach
    /// a `new`.
    fn is_call_only(&self, binding: &BindingId) -> bool {
        !self.dynamic_scope
            && !self.exported_declarations.contains(binding)
            && !self.is_shared_script_global(binding)
            && self.uses.has_single_declaration(binding)
            && self
                .uses
                .use_sites(binding)
                .iter()
                .all(|site| match &site.kind {
                    UseKind::CallCallee | UseKind::TypeofOperand => true,
                    UseKind::StaticMemberRead(name) => name == "call" || name == "apply",
                    _ => false,
                })
    }

    /// A binding the resolver placed in the input's top-level scope. In a
    /// script that scope is shared with every other script on the page.
    /// Functions in a top-level IIFE that a rule unwrapped keep their
    /// function-scope context, whose mark descends from the top-level mark.
    fn is_shared_script_global(&self, binding: &BindingId) -> bool {
        if self.is_es_module {
            return false;
        }
        let mark = binding.1.outer();
        mark == Mark::root() || mark.parent() == Mark::root()
    }

    fn is_global(&self, ident: &Ident) -> bool {
        ident.ctxt.outer() == self.unresolved_mark
    }

    /// Argument positions of `call` that receive a value the callee is known
    /// to only call.
    fn callback_positions(&self, call: &CallExpr) -> Vec<usize> {
        let Callee::Expr(callee) = &call.callee else {
            return Vec::new();
        };
        let spread_at = call
            .args
            .iter()
            .position(|arg| arg.spread.is_some())
            .unwrap_or(call.args.len());
        let mut positions = match crate::utils::paren::strip_parens(callee) {
            Expr::Ident(ident)
                if self.is_global(ident) && CALLBACK_GLOBALS.contains(&ident.sym.as_ref()) =>
            {
                vec![0]
            }
            Expr::Member(member) => static_member_name(&member.prop)
                .map(|name| builtin_callback_positions(&name).to_vec())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        if let Some(params) = self.callee_params(callee) {
            for (index, param) in params.iter().enumerate() {
                if param.as_ref().is_some_and(|param| self.is_call_only(param)) {
                    positions.push(index);
                }
            }
        }
        positions.retain(|index| *index < spread_at);
        positions
    }

    /// Parameters of a same-module function the call invokes directly: a
    /// literal callee or a binding that is declared once and never written.
    fn callee_params(&self, callee: &Expr) -> Option<Vec<Option<BindingId>>> {
        if self.dynamic_scope {
            return None;
        }
        match crate::utils::paren::strip_parens(callee) {
            Expr::Fn(function) => function_params(&function.function),
            Expr::Arrow(arrow) => Some(arrow.params.iter().map(pat_binding).collect()),
            Expr::Ident(ident) => {
                let binding = (ident.sym.clone(), ident.ctxt);
                let written = self
                    .uses
                    .use_sites(&binding)
                    .iter()
                    .any(|site| matches!(site.kind, UseKind::Write | UseKind::ReadWrite));
                if written || !self.uses.has_single_declaration(&binding) {
                    return None;
                }
                self.function_params.get(&binding).cloned()
            }
            _ => None,
        }
    }

    fn is_promise_executor(&self, new_expr: &NewExpr) -> bool {
        matches!(
            crate::utils::paren::strip_parens(&new_expr.callee),
            Expr::Ident(ident) if self.is_global(ident) && ident.sym == "Promise"
        )
    }
}

fn pat_binding(pat: &Pat) -> Option<BindingId> {
    match pat {
        Pat::Ident(binding) => Some((binding.id.sym.clone(), binding.id.ctxt)),
        _ => None,
    }
}

/// `None` when the function reads its own `arguments`: a parameter is then not
/// the only way the body reaches an argument.
fn function_params(function: &Function) -> Option<Vec<Option<BindingId>>> {
    let mut reads_arguments = HasArguments(false);
    visit_params_and_body(function, &mut reads_arguments);
    if reads_arguments.0 {
        return None;
    }
    Some(
        function
            .params
            .iter()
            .map(|param| pat_binding(&param.pat))
            .collect(),
    )
}

#[derive(Default)]
struct FunctionParamCollector {
    params: HashMap<BindingId, Vec<Option<BindingId>>>,
}

impl FunctionParamCollector {
    fn record(&mut self, name: &Ident, params: Option<Vec<Option<BindingId>>>) {
        if let Some(params) = params {
            self.params.insert((name.sym.clone(), name.ctxt), params);
        }
    }
}

impl Visit for FunctionParamCollector {
    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.record(&decl.ident, function_params(&decl.function));
        decl.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        if let (Pat::Ident(name), Some(init)) = (&decl.name, &decl.init) {
            match crate::utils::paren::strip_parens(init) {
                Expr::Fn(function) => self.record(&name.id, function_params(&function.function)),
                Expr::Arrow(arrow) => self.record(
                    &name.id,
                    Some(arrow.params.iter().map(pat_binding).collect()),
                ),
                _ => {}
            }
        }
        decl.visit_children_with(self);
    }
}

/// Bindings exported by their declaration (`export const f = ...`,
/// `export function f() {}`): a consumer module can construct them.
fn exported_declarations(module: &Module) -> HashSet<BindingId> {
    let mut exported = HashSet::default();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) = item else {
            continue;
        };
        match &export.decl {
            Decl::Var(var) => {
                for decl in &var.decls {
                    if let Some(binding) = pat_binding(&decl.name) {
                        exported.insert(binding);
                    }
                }
            }
            Decl::Fn(function) => {
                exported.insert((function.ident.sym.clone(), function.ident.ctxt));
            }
            _ => {}
        }
    }
    exported
}

struct ArrowFunctionConverter<'a> {
    evidence: &'a Evidence,
}

impl ArrowFunctionConverter<'_> {
    /// Converts the function a value position holds, through the wrappers
    /// that pass a value along unchanged.
    fn convert_value(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Paren(paren) => self.convert_value(&mut paren.expr),
            Expr::Seq(sequence) => {
                if let Some(last) = sequence.exprs.last_mut() {
                    self.convert_value(last);
                }
            }
            Expr::Cond(conditional) => {
                self.convert_value(&mut conditional.cons);
                self.convert_value(&mut conditional.alt);
            }
            Expr::Bin(binary)
                if matches!(
                    binary.op,
                    BinaryOp::LogicalOr | BinaryOp::LogicalAnd | BinaryOp::NullishCoalescing
                ) =>
            {
                self.convert_value(&mut binary.left);
                self.convert_value(&mut binary.right);
            }
            Expr::Fn(fn_expr) => {
                if let Some(arrow) = try_convert_to_arrow(fn_expr) {
                    *expr = Expr::Arrow(arrow);
                }
            }
            // `function () {}.bind(this)`
            Expr::Call(call) => {
                if let Some(arrow) = try_convert_bind_this(call) {
                    *expr = Expr::Arrow(arrow);
                }
            }
            _ => {}
        }
    }

    /// The invoked function of an IIFE: parentheses, a sequence's last
    /// expression, and the receiver of `.call` / `.apply`.
    fn convert_callee(&mut self, callee: &mut Expr) {
        match callee {
            Expr::Paren(paren) => self.convert_callee(&mut paren.expr),
            Expr::Seq(sequence) => {
                if let Some(last) = sequence.exprs.last_mut() {
                    self.convert_callee(last);
                }
            }
            Expr::Member(member)
                if static_member_name(&member.prop)
                    .is_some_and(|name| name == "call" || name == "apply") =>
            {
                self.convert_callee(&mut member.obj);
            }
            Expr::Fn(fn_expr) => {
                if let Some(arrow) = try_convert_to_arrow(fn_expr) {
                    *callee = Expr::Arrow(arrow);
                }
            }
            _ => {}
        }
    }
}

impl VisitMut for ArrowFunctionConverter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        // An async function has no [[Construct]] either.
        if let Expr::Fn(fn_expr) = expr {
            if fn_expr.function.is_async {
                if let Some(arrow) = try_convert_to_arrow(fn_expr) {
                    *expr = Expr::Arrow(arrow);
                }
            }
        }
    }

    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        // Decide on the pre-rewrite callee: a literal callee may itself
        // become an arrow.
        let callbacks = self.evidence.callback_positions(call);
        call.visit_mut_children_with(self);
        if let Callee::Expr(callee) = &mut call.callee {
            self.convert_callee(callee);
        }
        for index in callbacks {
            if let Some(arg) = call.args.get_mut(index) {
                self.convert_value(&mut arg.expr);
            }
        }
    }

    fn visit_mut_new_expr(&mut self, expr: &mut NewExpr) {
        expr.visit_mut_children_with(self);
        if self.evidence.is_promise_executor(expr) {
            if let Some(executor) = expr.args.as_mut().and_then(|args| args.first_mut()) {
                if executor.spread.is_none() {
                    self.convert_value(&mut executor.expr);
                }
            }
        }
    }

    fn visit_mut_var_declarator(&mut self, decl: &mut VarDeclarator) {
        decl.visit_mut_children_with(self);
        let Some(binding) = pat_binding(&decl.name) else {
            return;
        };
        if let Some(init) = &mut decl.init {
            if self.evidence.is_call_only(&binding) {
                self.convert_value(init);
            }
        }
    }

    fn visit_mut_key_value_prop(&mut self, prop: &mut KeyValueProp) {
        // Object property function values stay function expressions: a caller
        // of the property may construct it. Still recurse into the function
        // body so inner expressions are processed.
        prop.key.visit_mut_with(self);
        if let Expr::Fn(fn_expr) = prop.value.as_mut() {
            if let Some(body) = &mut fn_expr.function.body {
                body.visit_mut_with(self);
            }
        } else {
            prop.value.visit_mut_with(self);
        }
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

/// Converts every function expression that `try_convert_to_arrow` accepts,
/// without constructor evidence, except object property values. Only for
/// the private module copy Vue SFC recovery analyzes: Vue's template compiler
/// emits render functions, slots, and handlers as arrows, so a function there
/// is an ES5-lowered arrow. Never apply it to emitted JavaScript.
pub(crate) fn convert_lowered_vue_arrows(module: &mut Module) {
    struct Converter;
    impl VisitMut for Converter {
        fn visit_mut_expr(&mut self, expr: &mut Expr) {
            expr.visit_mut_children_with(self);
            match expr {
                Expr::Fn(fn_expr) => {
                    if let Some(arrow) = try_convert_to_arrow(fn_expr) {
                        *expr = Expr::Arrow(arrow);
                    }
                }
                Expr::Call(call) => {
                    if let Some(arrow) = try_convert_bind_this(call) {
                        *expr = Expr::Arrow(arrow);
                    }
                }
                _ => {}
            }
        }

        fn visit_mut_key_value_prop(&mut self, prop: &mut KeyValueProp) {
            prop.key.visit_mut_with(self);
            if let Expr::Fn(fn_expr) = prop.value.as_mut() {
                fn_expr.visit_mut_children_with(self);
            } else {
                prop.value.visit_mut_with(self);
            }
        }
    }
    module.visit_mut_with(&mut Converter);
}
