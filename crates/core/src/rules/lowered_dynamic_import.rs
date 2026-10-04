//! The `import(x)` that Babel, TypeScript, swc, sucrase, and rollup lower to
//! CommonJS as a wildcard interop of a `require` deferred to a promise
//! callback:
//!
//! ```js
//! Promise.resolve().then(() => _interopRequireWildcard(require("./a")))
//! Promise.resolve(spec).then((s) => _interopRequireWildcard(require(s)))
//! (s => new Promise(r => r(`${s}`)).then(s => _interopRequireWildcard(require(s))))(spec)
//! (function (t) { return Promise.resolve().then(function () { return _interopNamespaceDefault(require(t)); }); })(spec)
//! ```
//!
//! A non-literal specifier is passed through the promise or a wrapper call,
//! so it is evaluated before the callback runs, like the specifier of
//! `import()`. TypeScript before 4.5 and sucrase defer it into the callback;
//! `import()` evaluates it at the call again, as the source did. Callbacks
//! may be arrows or `function` expressions; a `function` callback must not
//! give the specifier its own `this`, `arguments`, or `eval` scope. See
//! `lowered_dynamic_import_source_semantics` in
//! `docs/rewrite-assumptions.md`.
//!
//! `UnEsm` restores the shape through the wildcard unwrap when the helper is
//! local. In unpack mode a helper bundled as its own module is known only
//! from cross-module facts, so Phase 2 runs
//! [`run_cross_module_lowered_dynamic_imports`].

use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowFunctionBody, CallExpr, Callee, Expr, ExprOrSpread, FnExpr, Ident, Import, MemberProp,
    Module, Pat, ReturnStmt, Stmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::arrow_function::has_this_or_arguments;
use super::cross_module_helper_refs::{
    collect_cross_module_helper_refs, cross_module_member_helper_kind,
};
use super::eval_utils::{has_dynamic_scope_construct, module_blocks_global_reference};
use super::helper_matcher::binding_key;
use super::transpiler_helper_utils::TranspilerHelperKind;
use super::un_interop_require_wildcard::is_canonical_interop_flag;
use crate::facts::ModuleFactsMap;
use crate::utils::paren::strip_parens;

/// Restores lowered `import()` calls whose wildcard helper is imported from
/// another module of the bundle, proven by that module's helper export
/// facts. Only a module `UnEsm` converted has such an import.
pub(crate) fn run_cross_module_lowered_dynamic_imports(
    module: &mut Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    unresolved_mark: Mark,
) {
    let refs = collect_cross_module_helper_refs(module, module_facts, current_filename, |kind| {
        kind == TranspilerHelperKind::InteropRequireWildcard
    });
    if refs.direct.is_empty() && refs.namespaces.is_empty() {
        return;
    }
    if module_blocks_global_reference(module, "Promise") {
        return;
    }
    let matcher = LoweredImportMatcher {
        is_wildcard_helper: |callee: &Expr| match callee {
            Expr::Ident(helper) => refs.direct.contains_key(&binding_key(helper)),
            callee => cross_module_member_helper_kind(callee, &refs.namespaces).is_some(),
        },
        unresolved_mark: Some(unresolved_mark),
    };
    module.visit_mut_with(&mut Restorer { matcher });
}

struct Restorer<F> {
    matcher: LoweredImportMatcher<F>,
}

impl<F: Fn(&Expr) -> bool> VisitMut for Restorer<F> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Some(import) = self.matcher.import_call(expr) {
            *expr = import;
        }
        expr.visit_mut_children_with(self);
    }
}

/// Matches lowered `import()` shapes around a wildcard interop helper that
/// `is_wildcard_helper` recognizes as a callee.
pub(crate) struct LoweredImportMatcher<F> {
    pub(crate) is_wildcard_helper: F,
    /// The resolver mark of free identifiers; `None` only for rule-level
    /// tests that run without one.
    pub(crate) unresolved_mark: Option<Mark>,
}

impl<F: Fn(&Expr) -> bool> LoweredImportMatcher<F> {
    /// `import(x)` for a lowered dynamic import of `x`.
    pub(crate) fn import_call(&self, expr: &Expr) -> Option<Expr> {
        let Expr::Call(call) = expr else {
            return None;
        };
        let specifier = self
            .wrapped_import_specifier(call)
            .or_else(|| self.lowered_import_specifier(call))?;
        Some(Expr::Call(CallExpr {
            span: call.span,
            callee: Callee::Import(Import {
                span: DUMMY_SP,
                phase: Default::default(),
            }),
            args: vec![ExprOrSpread {
                spread: None,
                expr: Box::new(specifier.clone()),
            }],
            ..Default::default()
        }))
    }

    /// The specifier of `<promise>.then(callback)`, where the callback
    /// returns the wildcard interop of `require(specifier)`.
    fn lowered_import_specifier<'a>(&self, then_call: &'a CallExpr) -> Option<&'a Expr> {
        let [callback] = then_call.args.as_slice() else {
            return None;
        };
        if callback.spread.is_some() {
            return None;
        }
        let passed = self.promise_resolution(member_call_object(&then_call.callee, "then")?)?;
        let (params, returned, is_function) = callback_return(&callback.expr)?;
        let specifier = self.wildcard_require_specifier(returned)?;
        match (passed, params.as_slice()) {
            (None, []) => (!(is_function
                && (has_this_or_arguments(specifier) || has_dynamic_scope_construct(specifier))))
            .then_some(specifier),
            (Some(passed), [Pat::Ident(param)]) => {
                reads_binding(specifier, &param.id).then_some(passed)
            }
            _ => None,
        }
    }

    /// The specifier passed to a one-parameter wrapper whose body is a
    /// lowered import of that parameter, or of the parameter converted to a
    /// string.
    fn wrapped_import_specifier<'a>(&self, call: &'a CallExpr) -> Option<&'a Expr> {
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let [specifier] = call.args.as_slice() else {
            return None;
        };
        let (params, Expr::Call(lowered), _) = callback_return(callee)? else {
            return None;
        };
        let [Pat::Ident(param)] = params.as_slice() else {
            return None;
        };
        let inner = self.lowered_import_specifier(lowered)?;
        let inner = match strip_parens(inner) {
            Expr::Tpl(tpl)
                if tpl.exprs.len() == 1 && tpl.quasis.iter().all(|quasi| quasi.raw.is_empty()) =>
            {
                &tpl.exprs[0]
            }
            inner => inner,
        };
        (specifier.spread.is_none() && reads_binding(inner, &param.id)).then_some(&specifier.expr)
    }

    /// What `Promise.resolve()`, `Promise.resolve(x)`, or
    /// `new Promise(r => r(x))` resolves with: `None` for nothing, `Some(x)`
    /// for `x`.
    fn promise_resolution<'a>(&self, expr: &'a Expr) -> Option<Option<&'a Expr>> {
        let is_promise = |expr: &Expr| matches!(strip_parens(expr), Expr::Ident(promise) if self.is_global(promise, "Promise"));
        match strip_parens(expr) {
            Expr::Call(call) => {
                if !is_promise(member_call_object(&call.callee, "resolve")?) {
                    return None;
                }
                match call.args.as_slice() {
                    [] => Some(None),
                    [value] if value.spread.is_none() => Some(Some(&value.expr)),
                    _ => None,
                }
            }
            Expr::New(new) if is_promise(&new.callee) => {
                let [executor] = new.args.as_deref()? else {
                    return None;
                };
                let (params, Expr::Call(call), _) = callback_return(&executor.expr)? else {
                    return None;
                };
                let [Pat::Ident(resolve)] = params.as_slice() else {
                    return None;
                };
                let Callee::Expr(callee) = &call.callee else {
                    return None;
                };
                match call.args.as_slice() {
                    [value]
                        if executor.spread.is_none()
                            && value.spread.is_none()
                            && reads_binding(callee, &resolve.id) =>
                    {
                        Some(Some(&value.expr))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// `x` of `helper(require(x))`, with the argument checks of the wildcard
    /// call unwrapping.
    fn wildcard_require_specifier<'a>(&self, expr: &'a Expr) -> Option<&'a Expr> {
        let Expr::Call(call) = strip_parens(expr) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        if !(self.is_wildcard_helper)(callee)
            || call.args.is_empty()
            || call.args.len() > 2
            || call.args.iter().any(|arg| arg.spread.is_some())
            || call
                .args
                .get(1)
                .is_some_and(|flag| !is_canonical_interop_flag(&flag.expr))
        {
            return None;
        }
        let Expr::Call(require) = call.args[0].expr.as_ref() else {
            return None;
        };
        let Callee::Expr(require_callee) = &require.callee else {
            return None;
        };
        let [specifier] = require.args.as_slice() else {
            return None;
        };
        (matches!(require_callee.as_ref(), Expr::Ident(id) if self.is_global(id, "require"))
            && specifier.spread.is_none())
        .then_some(specifier.expr.as_ref())
    }

    fn is_global(&self, id: &Ident, name: &str) -> bool {
        id.sym == name
            && self
                .unresolved_mark
                .is_none_or(|mark| id.ctxt.outer() == mark)
    }
}

/// The parameters and returned expression of a non-async, non-generator
/// arrow or function expression whose body only returns, and whether it is
/// a `function`.
fn callback_return(expr: &Expr) -> Option<(Vec<&Pat>, &Expr, bool)> {
    match strip_parens(expr) {
        Expr::Arrow(arrow) if !arrow.is_async && !arrow.is_generator => {
            let returned = match arrow.body.as_ref() {
                ArrowFunctionBody::Expr(expr) => expr.as_ref(),
                ArrowFunctionBody::FunctionBody(body) => single_return(&body.stmts)?,
            };
            Some((arrow.params.iter().collect(), strip_parens(returned), false))
        }
        Expr::Fn(FnExpr { function, .. }) if !function.is_async && !function.is_generator => {
            let returned = single_return(&function.body.as_ref()?.stmts)?;
            let params = function.params.iter().map(|param| &param.pat).collect();
            Some((params, strip_parens(returned), true))
        }
        _ => None,
    }
}

fn reads_binding(expr: &Expr, binding: &Ident) -> bool {
    matches!(strip_parens(expr), Expr::Ident(read) if read.to_id() == binding.to_id())
}

/// `obj` of a call to `obj.name(...)`.
fn member_call_object<'a>(callee: &'a Callee, name: &str) -> Option<&'a Expr> {
    let Callee::Expr(callee) = callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == name).then_some(&member.obj)
}

fn single_return(stmts: &[Stmt]) -> Option<&Expr> {
    match stmts {
        [Stmt::Return(ReturnStmt { arg: Some(arg), .. })] => Some(arg),
        _ => None,
    }
}
