//! The loaders webpack and Rspack emit for `import()` of another module of
//! the bundle, once unpack has resolved the module id to a path:
//!
//! ```js
//! Promise.resolve().then(() => require("./a.js"))           // webpack 5.111
//! Promise.resolve().then(require.bind(require, "./a.js"))   // webpack 5.107, Rspack
//! Promise.resolve().then(require.bind(null, "./a.js"))      // webpack 4
//! Promise.resolve().then(() => require.t("./a.js", 19))     // a CommonJS module
//! require.e(240).then(require.t.bind(require, "./a.js", 23))
//! Promise.all([require.e(1), require.e(2)]).then(() => require("./a.js"))
//! ```
//!
//! The promise is resolved, or the chunk loads behind `require.e` finished,
//! before the loader runs. With every chunk unpacked together the chunk
//! loads have nothing left to do, and the loader returns either the
//! module's exports object or, through `require.t(id, mode)`, webpack's fake
//! namespace of it:
//!
//! - `mode & 1`: the first argument is a module id to load (unpack rewrote
//!   it to a path); without it the argument is the module's value.
//! - `mode & 8`: return the exports object unchanged.
//! - `mode & 4`: return it unchanged when it carries `__esModule`.
//! - otherwise a new object whose `default` is the exports object, and with
//!   `mode & 2` a getter for each of its keys.
//!
//! [`run_cross_module_webpack_lazy_imports`] restores `import()` where the
//! provider's facts say which import gives that value; see
//! `webpack_lazy_import_source_semantics` in `docs/rewrite-assumptions.md`.
//! `KeptRequireFinder` leaves the require of these loaders to that pass, so
//! a consumer can still become an ES module in Phase 1.

use swc_core::atoms::Atom;
use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, CallExpr, Callee, Expr, ExprOrSpread, IdentName, Import, Lit,
    MemberExpr, MemberProp, Module, Pat, ReturnStmt, Stmt, Str,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::decl_utils::fresh_binding_ident;
use super::eval_utils::module_blocks_global_reference;
use crate::facts::{ModuleFacts, ModuleFactsMap};
use crate::provider_interface::{ProviderInterface, RequiredValueImport};
use crate::utils::paren::strip_parens;

/// What the loader of a lazy load returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LazyLoader {
    /// `require(path)`: the provider's exports object.
    Require,
    /// `require.t(path, mode)` with `mode & 1` set.
    FakeNamespace { mode: u32 },
}

/// A webpack lazy load of `specifier`.
pub(crate) struct LazyLoad<'a> {
    pub(crate) specifier: &'a Str,
    pub(crate) loader: LazyLoader,
}

/// The lazy load `call` performs when it is `<promise>.then(loader)` of one
/// of the forms in the module documentation, with a relative specifier.
/// `require`, and `Promise` where the promise reads it, must be the free
/// globals.
pub(crate) fn webpack_lazy_load(call: &CallExpr, unresolved_mark: Mark) -> Option<LazyLoad<'_>> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(MemberExpr {
        obj: promise,
        prop: MemberProp::Ident(then),
        ..
    }) = strip_parens(callee)
    else {
        return None;
    };
    if then.sym != "then" || !is_ready_promise(promise, unresolved_mark) {
        return None;
    }
    let [loader] = call.args.as_slice() else {
        return None;
    };
    if loader.spread.is_some() {
        return None;
    }
    let (specifier, loader) = loader_of(&loader.expr, unresolved_mark)?;
    let relative = specifier
        .value
        .as_str()
        .is_some_and(|value| value.starts_with("./") || value.starts_with("../"));
    relative.then_some(LazyLoad { specifier, loader })
}

/// Whether `call` is `<promise>.then(…)` on a chunk load, `require.e(chunk)`
/// or `Promise.all([require.e(chunk), …])`. A load whose chunk no input holds
/// keeps the `require.e` that [`kept_lazy_load_runtime_member`] reports.
pub(crate) fn then_waits_for_chunk_load(call: &CallExpr, unresolved_mark: Mark) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(MemberExpr {
        obj: promise,
        prop: MemberProp::Ident(then),
        ..
    }) = strip_parens(callee)
    else {
        return false;
    };
    then.sym == "then"
        && matches!(strip_parens(promise), Expr::Call(load) if is_chunk_load(load, unresolved_mark)
            || (is_ready_promise(promise, unresolved_mark)
                && global_member_call(load, "Promise", unresolved_mark) == Some("all")))
}

/// `Promise.resolve()`, `require.e(chunk)`, or
/// `Promise.all([require.e(chunk), …])`, with literal chunk ids.
fn is_ready_promise(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    if is_chunk_load(call, unresolved_mark) {
        return true;
    }
    let Some(method) = global_member_call(call, "Promise", unresolved_mark) else {
        return false;
    };
    match (method, call.args.as_slice()) {
        ("resolve", []) => true,
        ("all", [arg]) if arg.spread.is_none() => {
            matches!(strip_parens(&arg.expr), Expr::Array(array)
                if !array.elems.is_empty()
                    && array.elems.iter().all(|elem| matches!(elem, Some(ExprOrSpread { spread: None, expr })
                        if matches!(strip_parens(expr), Expr::Call(load) if is_chunk_load(load, unresolved_mark)))))
        }
        _ => false,
    }
}

/// `require.e(chunk)` with a literal chunk id.
fn is_chunk_load(call: &CallExpr, unresolved_mark: Mark) -> bool {
    global_member_call(call, "require", unresolved_mark) == Some("e")
        && matches!(call.args.as_slice(), [arg]
            if arg.spread.is_none()
                && matches!(strip_parens(&arg.expr), Expr::Lit(Lit::Num(_) | Lit::Str(_))))
}

/// The property name of a call `object.name(…)` on the free global `object`.
fn global_member_call<'a>(
    call: &'a CallExpr,
    object: &str,
    unresolved_mark: Mark,
) -> Option<&'a str> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(MemberExpr {
        obj,
        prop: MemberProp::Ident(prop),
        ..
    }) = strip_parens(callee)
    else {
        return None;
    };
    is_global(obj, object, unresolved_mark).then(|| prop.sym.as_ref())
}

fn is_global(expr: &Expr, name: &str, unresolved_mark: Mark) -> bool {
    matches!(strip_parens(expr), Expr::Ident(ident)
        if ident.sym == name && ident.ctxt.outer() == unresolved_mark)
}

/// The specifier and loader kind of a `.then` callback that loads a module.
fn loader_of(expr: &Expr, unresolved_mark: Mark) -> Option<(&Str, LazyLoader)> {
    match strip_parens(expr) {
        Expr::Call(bind) => bound_loader(bind, unresolved_mark),
        Expr::Arrow(arrow) if arrow.params.is_empty() && !arrow.is_async && !arrow.is_generator => {
            let call = match &*arrow.body {
                ArrowFunctionBody::Expr(body) => strip_parens(body),
                ArrowFunctionBody::FunctionBody(body) => single_returned(&body.stmts)?,
            };
            let Expr::Call(call) = call else {
                return None;
            };
            called_loader(call, unresolved_mark)
        }
        Expr::Fn(function)
            if function.function.params.is_empty()
                && !function.function.is_async
                && !function.function.is_generator =>
        {
            let body = function.function.body.as_ref()?;
            let Expr::Call(call) = single_returned(&body.stmts)? else {
                return None;
            };
            called_loader(call, unresolved_mark)
        }
        _ => None,
    }
}

fn single_returned(stmts: &[Stmt]) -> Option<&Expr> {
    let [Stmt::Return(ReturnStmt { arg: Some(arg), .. })] = stmts else {
        return None;
    };
    Some(strip_parens(arg))
}

/// `require(path)` or `require.t(path, mode)`.
fn called_loader(call: &CallExpr, unresolved_mark: Mark) -> Option<(&Str, LazyLoader)> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if is_global(callee, "require", unresolved_mark) {
        let [specifier] = call.args.as_slice() else {
            return None;
        };
        return Some((string_arg(specifier)?, LazyLoader::Require));
    }
    if global_member_call(call, "require", unresolved_mark) != Some("t") {
        return None;
    }
    let [specifier, mode] = call.args.as_slice() else {
        return None;
    };
    Some((string_arg(specifier)?, fake_namespace_loader(mode)?))
}

/// `require.bind(T, path)` or `require.t.bind(T, path, mode)`, with `T`
/// `require` or `null`.
fn bound_loader(call: &CallExpr, unresolved_mark: Mark) -> Option<(&Str, LazyLoader)> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(MemberExpr {
        obj: target,
        prop: MemberProp::Ident(bind),
        ..
    }) = strip_parens(callee)
    else {
        return None;
    };
    if bind.sym != "bind" {
        return None;
    }
    let this_arg = call.args.first()?;
    if this_arg.spread.is_some()
        || !(is_global(&this_arg.expr, "require", unresolved_mark)
            || matches!(strip_parens(&this_arg.expr), Expr::Lit(Lit::Null(_))))
    {
        return None;
    }
    if is_global(target, "require", unresolved_mark) {
        let [_, specifier] = call.args.as_slice() else {
            return None;
        };
        return Some((string_arg(specifier)?, LazyLoader::Require));
    }
    let Expr::Member(MemberExpr {
        obj,
        prop: MemberProp::Ident(t),
        ..
    }) = strip_parens(target)
    else {
        return None;
    };
    if t.sym != "t" || !is_global(obj, "require", unresolved_mark) {
        return None;
    }
    let [_, specifier, mode] = call.args.as_slice() else {
        return None;
    };
    Some((string_arg(specifier)?, fake_namespace_loader(mode)?))
}

fn string_arg(arg: &ExprOrSpread) -> Option<&Str> {
    match (&arg.spread, strip_parens(&arg.expr)) {
        (None, Expr::Lit(Lit::Str(value))) => Some(value),
        _ => None,
    }
}

/// The loader of `require.t(…, mode)` when the literal `mode` loads a module.
fn fake_namespace_loader(mode: &ExprOrSpread) -> Option<LazyLoader> {
    let Expr::Lit(Lit::Num(number)) = strip_parens(&mode.expr) else {
        return None;
    };
    if mode.spread.is_some() || number.value < 0.0 || number.value.fract() != 0.0 {
        return None;
    }
    let mode = number.value as u32;
    if mode & 1 == 0 {
        return None;
    }
    Some(if mode & 8 != 0 {
        LazyLoader::Require
    } else {
        LazyLoader::FakeNamespace { mode }
    })
}

/// How `import()` gives what the loader returned.
#[derive(Debug, PartialEq, Eq)]
enum ImportForm {
    /// `import(path)`.
    Namespace,
    /// `import(path).then((ns) => ns.default)`.
    Default,
}

/// `None` when no form gives the loader's value from what the provider
/// recovered as.
fn import_form(provider: &ModuleFacts, loader: LazyLoader) -> Option<ImportForm> {
    let provider = ProviderInterface::of(provider);
    let required_value = || match provider.required_value_import()? {
        RequiredValueImport::Namespace => Some(ImportForm::Namespace),
        RequiredValueImport::Default => Some(ImportForm::Default),
    };
    match loader {
        LazyLoader::Require => required_value(),
        // The marker makes `require.t` return the exports object itself.
        LazyLoader::FakeNamespace { mode } if mode & 4 != 0 && provider.marks_es_module() => {
            required_value()
        }
        // A fake namespace whose `default` is the exports object: the
        // namespace of an import whose default is that object.
        LazyLoader::FakeNamespace { .. } => match provider.required_value_import()? {
            RequiredValueImport::Default => Some(ImportForm::Namespace),
            RequiredValueImport::Namespace => None,
        },
    }
}

/// Restores `import()` from the webpack lazy loads of `module` whose
/// provider's facts give an import form, and drops their chunk loads.
pub(crate) fn run_cross_module_webpack_lazy_imports(
    module: &mut Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    unresolved_mark: Mark,
) {
    if module_blocks_global_reference(module, "Promise")
        || module_blocks_global_reference(module, "require")
    {
        return;
    }
    module.visit_mut_with(&mut Restorer {
        module_facts,
        current_filename,
        unresolved_mark,
    });
}

struct Restorer<'a> {
    module_facts: &'a ModuleFactsMap,
    current_filename: Option<&'a str>,
    unresolved_mark: Mark,
}

impl Restorer<'_> {
    fn restore(&self, call: &CallExpr) -> Option<Expr> {
        let load = webpack_lazy_load(call, self.unresolved_mark)?;
        let source = load.specifier.value.as_str()?;
        let provider = self.module_facts.get_from(self.current_filename, source)?;
        let import = Expr::Call(CallExpr {
            span: call.span,
            callee: Callee::Import(Import {
                span: DUMMY_SP,
                phase: Default::default(),
            }),
            args: vec![ExprOrSpread {
                spread: None,
                expr: Box::new(Expr::Lit(Lit::Str(load.specifier.clone()))),
            }],
            ..Default::default()
        });
        Some(match import_form(provider, load.loader)? {
            ImportForm::Namespace => import,
            ImportForm::Default => read_default(import),
        })
    }
}

impl VisitMut for Restorer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Call(call) = expr {
            if let Some(import) = self.restore(call) {
                *expr = import;
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }
}

/// The first webpack lazy-load runtime member `module` still calls on the
/// free `require`: `e` for `require.e(chunk)`, `t` for `require.t(…)` or
/// `require.t.bind(…)`. The split output defines neither, so a load that
/// [`run_cross_module_webpack_lazy_imports`] kept throws when it runs.
pub(crate) fn kept_lazy_load_runtime_member(
    module: &Module,
    unresolved_mark: Mark,
) -> Option<&'static str> {
    let mut finder = RuntimeMemberFinder {
        unresolved_mark,
        found: None,
    };
    module.visit_with(&mut finder);
    finder.found
}

struct RuntimeMemberFinder {
    unresolved_mark: Mark,
    found: Option<&'static str>,
}

impl Visit for RuntimeMemberFinder {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if self.found.is_some() {
            return;
        }
        self.found = match global_member_call(call, "require", self.unresolved_mark) {
            Some("e") => Some("e"),
            Some("t") => Some("t"),
            _ => match &call.callee {
                Callee::Expr(callee) => match strip_parens(callee) {
                    Expr::Member(MemberExpr {
                        obj,
                        prop: MemberProp::Ident(bind),
                        ..
                    }) if bind.sym == "bind" => match strip_parens(obj) {
                        Expr::Member(MemberExpr {
                            obj,
                            prop: MemberProp::Ident(t),
                            ..
                        }) if t.sym == "t" && is_global(obj, "require", self.unresolved_mark) => {
                            Some("t")
                        }
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            },
        };
        call.visit_children_with(self);
    }
}

/// `promise.then((ns) => ns.default)`.
fn read_default(promise: Expr) -> Expr {
    let ns = fresh_binding_ident(Atom::from("ns"), DUMMY_SP);
    let callback = Expr::Arrow(ArrowExpr {
        span: DUMMY_SP,
        params: vec![Pat::Ident(ns.clone().into())],
        body: Box::new(ArrowFunctionBody::Expr(Box::new(Expr::Member(
            MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Ident(ns)),
                prop: MemberProp::Ident(IdentName::new(Atom::from("default"), DUMMY_SP)),
            },
        )))),
        ..Default::default()
    });
    Expr::Call(CallExpr {
        span: DUMMY_SP,
        callee: Callee::Expr(Box::new(Expr::Member(MemberExpr {
            span: DUMMY_SP,
            obj: Box::new(promise),
            prop: MemberProp::Ident(IdentName::new(Atom::from("then"), DUMMY_SP)),
        }))),
        args: vec![ExprOrSpread {
            spread: None,
            expr: Box::new(callback),
        }],
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{ExportFact, ExportKind};

    fn facts(configure: impl FnOnce(&mut ModuleFacts)) -> ModuleFacts {
        let mut facts = ModuleFacts::default();
        configure(&mut facts);
        facts
    }

    fn default_export(facts: &mut ModuleFacts) {
        facts.commonjs_value_is_default = true;
        facts.exports.push(ExportFact {
            exported: Atom::from("default"),
            local: None,
            kind: ExportKind::Default,
        });
    }

    #[test]
    fn plain_require_reads_the_namespace_of_an_es_module() {
        let provider = facts(|facts| {
            facts.exports.push(ExportFact {
                exported: Atom::from("label"),
                local: Some(Atom::from("label")),
                kind: ExportKind::Named,
            });
        });
        assert_eq!(
            import_form(&provider, LazyLoader::Require),
            Some(ImportForm::Namespace)
        );
    }

    #[test]
    fn plain_require_reads_the_default_of_a_commonjs_value() {
        let provider = facts(default_export);
        assert_eq!(
            import_form(&provider, LazyLoader::Require),
            Some(ImportForm::Default)
        );
        let kept = facts(|facts| facts.stays_commonjs = true);
        assert_eq!(
            import_form(&kept, LazyLoader::Require),
            Some(ImportForm::Default)
        );
    }

    #[test]
    fn fake_namespace_of_a_commonjs_value_is_the_import_namespace() {
        let provider = facts(default_export);
        for mode in [17, 19, 23, 7] {
            assert_eq!(
                import_form(&provider, LazyLoader::FakeNamespace { mode }),
                Some(ImportForm::Namespace),
                "mode {mode}"
            );
        }
    }

    #[test]
    fn fake_namespace_of_a_marked_provider_follows_mode_4() {
        let provider = facts(|facts| {
            facts.marks_es_module = true;
            facts.exports.push(ExportFact {
                exported: Atom::from("label"),
                local: Some(Atom::from("label")),
                kind: ExportKind::Named,
            });
        });
        // `mode & 4` returns the marked exports object: the namespace.
        assert_eq!(
            import_form(&provider, LazyLoader::FakeNamespace { mode: 23 }),
            Some(ImportForm::Namespace)
        );
        // Otherwise its `default` is the whole exports object, which no
        // import of the recovered module gives.
        assert_eq!(
            import_form(&provider, LazyLoader::FakeNamespace { mode: 19 }),
            None
        );
    }
}
