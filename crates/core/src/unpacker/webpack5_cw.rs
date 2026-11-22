//! webpack 5's CommonJS wrapper for scope-hoisted CommonJS modules.
//!
//! Since 5.110.0, `optimization.concatenateModules` can hoist a CommonJS
//! module into the concatenated root module. The module body becomes a
//! function wrapped by the `concatenation wrap` runtime:
//!
//! ```js
//! __webpack_require__.cw = (body) => {
//!   var mod;
//!   return () => {
//!     if (body) {
//!       var fn = body;
//!       body = 0;
//!       mod = { exports: {} };
//!       fn.call(mod.exports, mod, mod.exports);
//!     }
//!     return mod.exports;
//!   };
//! };
//! var provider_namespaceFn = __webpack_require__.cw(function(module, exports) { … });
//! provider_namespaceFn();
//! ```
//!
//! The accessor runs the body once, on its first call, with `this` and the
//! second argument set to the exports object and the first to a module
//! object that exists before the body runs, and returns `mod.exports`. That
//! is CommonJS `require()` of the body as a module file: each wrapped body
//! becomes its own CommonJS module, and every call of the accessor becomes
//! `require("./<file>")`. The rest of the bootstrap becomes `entry.js`.
//!
//! The runtime is recognized by its shape, never by name: assigned to a
//! property of a require-scope object (`R.cw = …`), held in an object literal
//! property (`const R = { cw: … }`), bound to its own variable
//! (`const t = …`, Terser with several wrapped modules), or inlined into its
//! only call (`var r = (…)(function(r, t) { … })`, Terser with one).
//!
//! A bootstrap that also has a module table splits through the table path
//! instead, with each wrapped body as one more module of the table
//! ([`split_table_wrappers`]).

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, AssignOp, AssignTarget, CallExpr, Callee, Decl, Expr, ExprStmt,
    Function, FunctionBody, Id, Ident, Lit, MemberExpr, MemberProp, Module, ModuleItem, ObjectLit,
    Pat, Prop, PropName, PropOrSpread, SimpleAssignTarget, Stmt, Str, VarDecl, VarDeclarator,
};
use swc_core::ecma::codegen::Config;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::collections::{HashMap, HashSet};
use crate::module_path::relative_import_specifier;
use crate::rules::eval_utils::node_has_dynamic_scope_construct;
use crate::unpacker::emit_esm::{dedup_filename, FilenameDedupStyle};
use crate::unpacker::refusal::{self, because, SplitRefusalReason};
use crate::unpacker::surrounding::{IdentNameCollector, SurroundingCode, SurroundingItems};
use crate::unpacker::webpack5::{prepare_translated_webpack_factories, TranslatedWebpackFactory};
use crate::unpacker::webpack_common::webpack_module_filename;
use crate::unpacker::{
    emit_module_with_positions, source_fallback_for_stmts, spans_byte_ranges,
    stmts_have_function_level_arguments_or_new_target, stmts_have_function_level_return,
    stmts_have_function_level_special_bindings, BundleFormat, DetectedBundle, SourcePositions,
    UnpackResult, UnpackedModule,
};
use crate::utils::paren::strip_parens;
use crate::utils::swc_safety::apply_fixer;

const ENTRY_FILENAME: &str = "entry.js";

/// Free names whose meaning `entry.js` changes: it calls `require`, and as a
/// CommonJS module it has its own `module` and `exports`. Code around the
/// bootstrap that declares one of them keeps the input one file.
const ENTRY_FREE_NAMES: [&str; 3] = ["require", "module", "exports"];

/// Split a webpack 5 bootstrap whose concatenated root holds CommonJS
/// modules wrapped by the `.cw` runtime. `module_items[bundle_idx]` is the
/// statement holding the bootstrap IIFE whose body is `bootstrap_body`.
///
/// `None` when the bootstrap has no wrapped module, or when one of them
/// cannot leave it without changing behavior (the input then stays one file):
/// an accessor used other than as a zero-argument call, a body that reads a
/// bootstrap binding other than another accessor, a body that observes its
/// function boundary (`arguments`, `new.target`, `return`, and `this` in an
/// arrow body), bootstrap code that does, or a binding named `require`.
pub(super) fn detect_from_bootstrap(
    module_items: &[ModuleItem],
    bundle_idx: usize,
    bootstrap_body: &FunctionBody,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> Option<DetectedBundle> {
    // Every IIFE webpack's table scan rejects reaches here; keep the miss
    // cheap and decide on the top-level statements before cloning anything.
    if !bootstrap_body.stmts.iter().any(stmt_holds_runtime) {
        return None;
    }
    let span = tracing::info_span!("webpack5: detect_cw");
    let _enter = span.enter();

    let mut analysis = Module {
        span: DUMMY_SP,
        body: bootstrap_body
            .stmts
            .iter()
            .cloned()
            .map(ModuleItem::Stmt)
            .collect(),
        shebang: None,
    };
    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    analysis.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
    let top_ctxt = SyntaxContext::empty().apply_mark(top_level_mark);

    let stmts: Vec<&Stmt> = analysis
        .body
        .iter()
        .filter_map(|item| match item {
            ModuleItem::Stmt(stmt) => Some(stmt),
            ModuleItem::ModuleDecl(_) => None,
        })
        .collect();
    let runtimes = collect_runtime_sites(&stmts, top_ctxt);
    let wrappers = collect_wrappers(&stmts, &runtimes, top_ctxt);
    if wrappers.is_empty() {
        return None;
    }

    // The runtime and an accessor built by it prove the format; any failure
    // from here on refuses a webpack 5 bundle.
    let commitment = refusal::commit(BundleFormat::Webpack5);

    because(
        accessors_are_exclusive(&analysis, &runtimes, &wrappers),
        SplitRefusalReason::Other,
    )?;
    because(
        (!binds_require(&analysis, unresolved_mark)
            && !node_has_dynamic_scope_construct(&analysis))
        .then_some(()),
        SplitRefusalReason::Other,
    )?;
    let wrapper_ids: HashSet<Id> = wrappers.iter().map(|wrapper| wrapper.id.clone()).collect();
    for wrapper in &wrappers {
        let factory = wrapper_factory(&stmts, wrapper).expect("collected wrapper has a factory");
        because(
            factory_is_movable(&factory, &wrapper_ids, top_ctxt),
            SplitRefusalReason::FactoryRecovery,
        )?;
    }

    let strict = bootstrap_is_strict(module_items, bundle_idx, bootstrap_body);
    let filenames = wrapper_filenames(&cm, &stmts, &wrappers);
    let id_to_filename: HashMap<Id, String> = wrappers
        .iter()
        .zip(&filenames)
        .map(|(wrapper, filename)| (wrapper.id.clone(), filename.clone()))
        .collect();
    let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);

    let mut modules = Vec::with_capacity(wrappers.len() + 1);
    let mut translated = Vec::with_capacity(wrappers.len());
    let mut factory_spans = Vec::with_capacity(wrappers.len());
    let mut settled: HashSet<Id> = HashSet::default();
    let mut loads: HashMap<Id, Vec<Id>> = HashMap::default();
    for (index, (wrapper, filename)) in wrappers.iter().zip(&filenames).enumerate() {
        let factory = wrapper_factory(&stmts, wrapper).expect("collected wrapper has a factory");
        factory_spans.push(factory.span);
        if exports_settle_on_load(&factory) {
            settled.insert(wrapper.id.clone());
        }
        loads.insert(
            wrapper.id.clone(),
            unconditional_loads(&factory.body, &wrapper_ids),
        );
        modules.push(UnpackedModule {
            id: filename.clone(),
            is_entry: false,
            code: source_fallback_for_stmts(&cm, &factory.body),
            filename: filename.clone(),
            source_ranges: spans_byte_ranges(&cm, factory.body.iter().map(Spanned::span)),
            ..Default::default()
        });
        let mut body = factory.body;
        let mut params = factory.params;
        let mut rewriter = AccessorCallRewriter {
            targets: &id_to_filename,
            bound: &HashSet::default(),
            from_filename: filename,
            require_ctxt: unresolved_ctxt,
        };
        body.visit_mut_with(&mut rewriter);
        // The body ran inside the strict bootstrap; as its own file it needs
        // the directive to stay strict.
        if strict && !has_use_strict_directive(&body) {
            body.insert(0, use_strict_directive());
        }
        body.visit_mut_with(&mut ClearContexts);
        params.visit_mut_with(&mut ClearContexts);
        translated.push(TranslatedWebpackFactory {
            // Not a request any authored `require("…")` can spell.
            id: format!("cw:{index}"),
            filename: filename.clone(),
            params,
            body,
        });
    }
    let (mut prepared, failures) = because(
        prepare_translated_webpack_factories(&translated),
        SplitRefusalReason::FactoryRecovery,
    )?;
    // An opaque module would keep its raw body, which still calls the
    // accessors of the bootstrap it left.
    because(
        failures.is_empty().then_some(()),
        SplitRefusalReason::FactoryRecovery,
    )?;

    let surrounding = because(
        SurroundingCode::collect(
            SurroundingItems::around(module_items, bundle_idx),
            &ENTRY_FREE_NAMES,
            &factory_spans,
        ),
        SplitRefusalReason::SurroundingCode,
    )?;
    because(
        (!surrounding.has_prelude()).then_some(()),
        SplitRefusalReason::SurroundingCode,
    )?;

    let mut entry_stmts = entry_statements(&analysis, &runtimes, &wrappers);
    because(
        (!stmts_have_function_level_special_bindings(&entry_stmts)
            && !stmts_have_function_level_return(&entry_stmts))
        .then_some(()),
        SplitRefusalReason::Other,
    )?;
    let scope_objects: HashSet<Id> = wrappers
        .iter()
        .filter_map(|wrapper| match &wrapper.callee {
            WrapperCallee::Host(RuntimeHost::Member(object, _)) => Some(object.clone()),
            _ => None,
        })
        .collect();
    for object in &scope_objects {
        drop_unused_require_scope(&mut entry_stmts, object, top_ctxt);
    }
    // Code in entry.js that reads an `__esModule` marker (webpack's `.n`
    // interop helper, left as local code) needs the provider's exports
    // object as it is; a bound accessor lets the pipeline turn the provider
    // into an ES module, whose namespace has no marker. The calls then stay
    // `require()` calls.
    let bound = if reads_es_module_marker(&entry_stmts) {
        HashSet::default()
    } else {
        bind_import_stubs(
            &mut entry_stmts,
            &settled,
            &loads,
            &id_to_filename,
            unresolved_ctxt,
            top_ctxt,
        )
    };
    let mut rewriter = AccessorCallRewriter {
        targets: &id_to_filename,
        bound: &bound,
        from_filename: ENTRY_FILENAME,
        require_ctxt: unresolved_ctxt,
    };
    entry_stmts.visit_mut_with(&mut rewriter);
    if strict && !has_use_strict_directive(&entry_stmts) {
        entry_stmts.insert(0, use_strict_directive());
    }
    let entry_ranges = spans_byte_ranges(
        &cm,
        entry_stmts
            .iter()
            .map(Spanned::span)
            .chain(surrounding.spans()),
    );
    let mut entry_module = Module {
        span: DUMMY_SP,
        body: entry_stmts.into_iter().map(ModuleItem::Stmt).collect(),
        shebang: None,
    };
    surrounding.rename_conflicts(&mut entry_module, top_level_mark);
    because(
        surrounding.place_around(&mut entry_module, ENTRY_FILENAME),
        SplitRefusalReason::SurroundingCode,
    )?;
    apply_fixer(&mut entry_module).ok()?;
    let entry_code = emit_module_with_positions(
        &entry_module,
        cm.clone(),
        Config::default().with_minify(false),
        positions,
    )
    .ok()?;
    modules.push(UnpackedModule {
        id: "entry".to_string(),
        is_entry: true,
        code: entry_code.code,
        filename: ENTRY_FILENAME.to_string(),
        source_ranges: entry_ranges,
        generated_source_map: entry_code.points,
        ..Default::default()
    });
    prepared.push(None);

    commitment.accept(Some(DetectedBundle::new(
        UnpackResult::new(modules, BundleFormat::Webpack5),
        prepared,
        cm,
    )))
}

// ---------------------------------------------------------------------------
// Runtime shape
// ---------------------------------------------------------------------------

/// Whether `expr` is the `.cw` runtime: a one-parameter function returning a
/// memoizing accessor that, on its first call, clears the parameter, creates
/// `{ exports: {} }`, and calls the parameter with it as
/// `fn.call(mod.exports, mod, mod.exports)`, then returns `mod.exports`.
/// Terser's sequences are flattened, so the minified and unminified
/// spellings match alike.
pub(super) fn is_cw_runtime(expr: &Expr) -> bool {
    let Some((params, body)) = plain_function_parts(expr) else {
        return false;
    };
    let [Pat::Ident(body_param)] = params.as_slice() else {
        return false;
    };
    let body_sym = &body_param.id.sym;
    let Some(body) = body else {
        return false;
    };
    let [Stmt::Decl(Decl::Var(mod_decl)), Stmt::Return(ret)] = body.as_slice() else {
        return false;
    };
    let Some(mod_sym) = sole_declarator(mod_decl)
        .filter(|decl| decl.init.is_none())
        .and_then(declarator_sym)
    else {
        return false;
    };
    let Some(accessor) = ret.arg.as_deref() else {
        return false;
    };
    let Some((accessor_params, Some(accessor_body))) = plain_function_parts(accessor) else {
        return false;
    };
    if !accessor_params.is_empty() {
        return false;
    }
    let [Stmt::If(guard), Stmt::Return(result)] = accessor_body.as_slice() else {
        return false;
    };
    if guard.alt.is_some()
        || !is_ident_named(&guard.test, body_sym)
        || !result
            .arg
            .as_deref()
            .is_some_and(|arg| is_member_named(arg, mod_sym, "exports"))
    {
        return false;
    }
    let Stmt::Block(block) = &*guard.cons else {
        return false;
    };
    let steps = flatten_steps(&block.stmts);
    let [Step::Var(fn_decl), Step::Expr(clear), Step::Expr(create), Step::Expr(call)] =
        steps.as_slice()
    else {
        return false;
    };
    let Some(fn_sym) = sole_declarator(fn_decl)
        .filter(|decl| {
            decl.init
                .as_deref()
                .is_some_and(|init| is_ident_named(init, body_sym))
        })
        .and_then(declarator_sym)
    else {
        return false;
    };
    if fn_sym == body_sym || fn_sym == mod_sym || body_sym == mod_sym {
        return false;
    }
    is_assign_to(
        clear,
        body_sym,
        |value| matches!(value, Expr::Lit(Lit::Num(number)) if number.value == 0.0),
    ) && is_assign_to(create, mod_sym, is_empty_exports_object)
        && is_module_call(call, fn_sym, mod_sym)
}

/// Params and body statements of a plain (sync, non-generator) function or
/// arrow; an arrow's expression body has no statements.
fn plain_function_parts(expr: &Expr) -> Option<(Vec<&Pat>, Option<&Vec<Stmt>>)> {
    match strip_parens(expr) {
        Expr::Arrow(arrow) if !arrow.is_async && !arrow.is_generator => Some((
            arrow.params.iter().collect(),
            match &*arrow.body {
                ArrowFunctionBody::FunctionBody(body) => Some(&body.stmts),
                ArrowFunctionBody::Expr(_) => None,
            },
        )),
        Expr::Fn(function) if is_plain_function(&function.function) => Some((
            function
                .function
                .params
                .iter()
                .map(|param| &param.pat)
                .collect(),
            function.function.body.as_ref().map(|body| &body.stmts),
        )),
        _ => None,
    }
}

fn is_plain_function(function: &Function) -> bool {
    !function.is_async && !function.is_generator && function.body.is_some()
}

enum Step<'a> {
    Var(&'a VarDecl),
    Expr(&'a Expr),
}

fn flatten_steps(stmts: &[Stmt]) -> Vec<Step<'_>> {
    let mut steps = Vec::new();
    for stmt in stmts {
        match stmt {
            Stmt::Decl(Decl::Var(var)) => steps.push(Step::Var(var)),
            Stmt::Expr(ExprStmt { expr, .. }) => match strip_parens(expr) {
                Expr::Seq(seq) => steps.extend(seq.exprs.iter().map(|expr| Step::Expr(expr))),
                expr => steps.push(Step::Expr(expr)),
            },
            // Anything else is not the runtime; make the match fail.
            _ => steps.push(Step::Expr(&INVALID)),
        }
    }
    steps
}

static INVALID: Expr = Expr::Invalid(swc_core::ecma::ast::Invalid { span: DUMMY_SP });

fn sole_declarator(var: &VarDecl) -> Option<&VarDeclarator> {
    match var.decls.as_slice() {
        [decl] => Some(decl),
        _ => None,
    }
}

fn declarator_sym(decl: &VarDeclarator) -> Option<&Atom> {
    match &decl.name {
        Pat::Ident(binding) => Some(&binding.id.sym),
        _ => None,
    }
}

fn is_ident_named(expr: &Expr, sym: &Atom) -> bool {
    matches!(strip_parens(expr), Expr::Ident(ident) if &ident.sym == sym)
}

fn is_member_named(expr: &Expr, object: &Atom, prop: &str) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    is_ident_named(&member.obj, object) && static_prop_name(&member.prop).is_some_and(|p| p == prop)
}

fn is_assign_to(expr: &Expr, target: &Atom, value: impl Fn(&Expr) -> bool) -> bool {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return false;
    };
    assign.op == AssignOp::Assign
        && matches!(
            &assign.left,
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) if &binding.id.sym == target
        )
        && value(strip_parens(&assign.right))
}

fn is_empty_exports_object(expr: &Expr) -> bool {
    let Expr::Object(object) = expr else {
        return false;
    };
    let [PropOrSpread::Prop(prop)] = object.props.as_slice() else {
        return false;
    };
    let Prop::KeyValue(kv) = &**prop else {
        return false;
    };
    prop_name_str(&kv.key).is_some_and(|key| key == "exports")
        && matches!(strip_parens(&kv.value), Expr::Object(ObjectLit { props, .. }) if props.is_empty())
}

/// `fn.call(mod.exports, mod, mod.exports)`.
fn is_module_call(expr: &Expr, fn_sym: &Atom, mod_sym: &Atom) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !is_member_named(callee, fn_sym, "call") {
        return false;
    }
    let [this_arg, module_arg, exports_arg] = call.args.as_slice() else {
        return false;
    };
    this_arg.spread.is_none()
        && module_arg.spread.is_none()
        && exports_arg.spread.is_none()
        && is_member_named(&this_arg.expr, mod_sym, "exports")
        && is_ident_named(&module_arg.expr, mod_sym)
        && is_member_named(&exports_arg.expr, mod_sym, "exports")
}

fn static_prop_name(prop: &MemberProp) -> Option<&str> {
    match prop {
        MemberProp::Ident(name) => Some(name.sym.as_str()),
        MemberProp::Computed(computed) => match &*computed.expr {
            Expr::Lit(Lit::Str(value)) => value.value.as_str(),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

fn prop_name_str(key: &PropName) -> Option<&str> {
    match key {
        PropName::Ident(name) => Some(name.sym.as_str()),
        PropName::Str(value) => value.value.as_str(),
        _ => None,
    }
}

/// Shallow precheck on the unresolved bootstrap: does a top-level statement
/// hold the runtime in one of the positions [`collect_runtime_sites`] and
/// [`collect_wrappers`] accept?
fn stmt_holds_runtime(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => {
            assign_elements(expr).any(|(_, assign_right)| is_cw_runtime(assign_right))
        }
        Stmt::Decl(Decl::Var(var)) => var.decls.iter().any(|decl| {
            let Some(init) = decl.init.as_deref() else {
                return false;
            };
            match strip_parens(init) {
                Expr::Object(object) => object_runtime_props(object).next().is_some(),
                Expr::Call(call) => matches!(
                    &call.callee,
                    Callee::Expr(callee) if is_cw_runtime(callee)
                ),
                init => is_cw_runtime(init),
            }
        }),
        _ => false,
    }
}

/// The `target = value` elements of an expression statement, by position in
/// its comma sequence (0 for a lone expression).
fn assign_elements(expr: &Expr) -> impl Iterator<Item = (&MemberExpr, &Expr)> {
    let elements: Vec<&Expr> = match strip_parens(expr) {
        Expr::Seq(seq) => seq.exprs.iter().map(|expr| &**expr).collect(),
        expr => vec![expr],
    };
    elements.into_iter().filter_map(|element| {
        let Expr::Assign(assign) = strip_parens(element) else {
            return None;
        };
        if assign.op != AssignOp::Assign {
            return None;
        }
        let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
            return None;
        };
        Some((member, &*assign.right))
    })
}

fn object_runtime_props(object: &ObjectLit) -> impl Iterator<Item = &str> {
    object.props.iter().filter_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(kv) = &**prop else {
            return None;
        };
        is_cw_runtime(&kv.value)
            .then(|| prop_name_str(&kv.key))
            .flatten()
    })
}

// ---------------------------------------------------------------------------
// Runtime sites and wrapped modules
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq)]
enum RuntimeHost {
    /// `R.<prop>` of a bootstrap object `R`.
    Member(Id, Atom),
    /// A bootstrap binding holding the runtime.
    Binding(Id),
}

struct RuntimeSite {
    host: RuntimeHost,
    stmt: usize,
    /// For a member host: whether an assignment statement installs it (as
    /// opposed to an object literal property).
    assigned: bool,
}

fn collect_runtime_sites(stmts: &[&Stmt], top_ctxt: SyntaxContext) -> Vec<RuntimeSite> {
    let mut sites = Vec::new();
    for (index, stmt) in stmts.iter().enumerate() {
        match stmt {
            Stmt::Expr(ExprStmt { expr, .. }) => {
                for (member, value) in assign_elements(expr) {
                    let Expr::Ident(object) = &*member.obj else {
                        continue;
                    };
                    let Some(prop) = static_prop_name(&member.prop) else {
                        continue;
                    };
                    if object.ctxt == top_ctxt && is_cw_runtime(value) {
                        sites.push(RuntimeSite {
                            host: RuntimeHost::Member(object.to_id(), Atom::from(prop)),
                            stmt: index,
                            assigned: true,
                        });
                    }
                }
            }
            Stmt::Decl(Decl::Var(var)) => {
                for decl in &var.decls {
                    let (Pat::Ident(name), Some(init)) = (&decl.name, decl.init.as_deref()) else {
                        continue;
                    };
                    if name.id.ctxt != top_ctxt {
                        continue;
                    }
                    match strip_parens(init) {
                        Expr::Object(object) => {
                            for prop in object_runtime_props(object) {
                                sites.push(RuntimeSite {
                                    host: RuntimeHost::Member(name.id.to_id(), Atom::from(prop)),
                                    stmt: index,
                                    assigned: false,
                                });
                            }
                        }
                        init if is_cw_runtime(init) => sites.push(RuntimeSite {
                            host: RuntimeHost::Binding(name.id.to_id()),
                            stmt: index,
                            assigned: false,
                        }),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    sites
}

#[derive(Clone)]
enum WrapperCallee {
    Host(RuntimeHost),
    Inline,
}

struct Wrapper {
    id: Id,
    stmt: usize,
    decl: usize,
    callee: WrapperCallee,
}

/// Top-level declarators `X = <runtime>(<function>)`.
fn collect_wrappers(
    stmts: &[&Stmt],
    runtimes: &[RuntimeSite],
    top_ctxt: SyntaxContext,
) -> Vec<Wrapper> {
    let mut wrappers = Vec::new();
    for (index, stmt) in stmts.iter().enumerate() {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for (decl_index, decl) in var.decls.iter().enumerate() {
            let (Pat::Ident(name), Some(init)) = (&decl.name, decl.init.as_deref()) else {
                continue;
            };
            if name.id.ctxt != top_ctxt {
                continue;
            }
            let Expr::Call(call) = strip_parens(init) else {
                continue;
            };
            if factory_arg(call).is_none() {
                continue;
            }
            let Callee::Expr(callee) = &call.callee else {
                continue;
            };
            let host = match strip_parens(callee) {
                Expr::Member(member) => match (&*member.obj, static_prop_name(&member.prop)) {
                    (Expr::Ident(object), Some(prop)) => {
                        Some(RuntimeHost::Member(object.to_id(), Atom::from(prop)))
                    }
                    _ => None,
                },
                Expr::Ident(ident) => Some(RuntimeHost::Binding(ident.to_id())),
                _ => None,
            };
            let callee = match host {
                Some(host) if runtimes.iter().any(|site| site.host == host) => {
                    WrapperCallee::Host(host)
                }
                Some(_) => continue,
                None if is_cw_runtime(callee) => WrapperCallee::Inline,
                None => continue,
            };
            wrappers.push(Wrapper {
                id: name.id.to_id(),
                stmt: index,
                decl: decl_index,
                callee,
            });
        }
    }
    wrappers
}

/// The wrapped function of `runtime(<function>)`.
fn factory_arg(call: &CallExpr) -> Option<&Expr> {
    let [arg] = call.args.as_slice() else {
        return None;
    };
    if arg.spread.is_some() {
        return None;
    }
    match strip_parens(&arg.expr) {
        expr @ (Expr::Arrow(_) | Expr::Fn(_)) => Some(expr),
        _ => None,
    }
}

struct Factory {
    params: Vec<Pat>,
    body: Vec<Stmt>,
    is_arrow: bool,
    span: Span,
}

fn wrapper_factory(stmts: &[&Stmt], wrapper: &Wrapper) -> Option<Factory> {
    let Stmt::Decl(Decl::Var(var)) = stmts[wrapper.stmt] else {
        return None;
    };
    let Expr::Call(call) = strip_parens(var.decls[wrapper.decl].init.as_deref()?) else {
        return None;
    };
    function_factory(factory_arg(call)?)
}

/// The parts of a wrapped function `factory_arg` returned.
fn function_factory(function: &Expr) -> Option<Factory> {
    match function {
        Expr::Arrow(ArrowExpr {
            params,
            body,
            is_async: false,
            is_generator: false,
            span,
            ..
        }) => Some(Factory {
            params: params.clone(),
            body: match &**body {
                ArrowFunctionBody::FunctionBody(block) => block.stmts.clone(),
                ArrowFunctionBody::Expr(expr) => vec![Stmt::Expr(ExprStmt {
                    span: expr.span(),
                    expr: expr.clone(),
                })],
            },
            is_arrow: true,
            span: *span,
        }),
        Expr::Fn(function) if is_plain_function(&function.function) => Some(Factory {
            params: function
                .function
                .params
                .iter()
                .map(|param| param.pat.clone())
                .collect(),
            body: function.function.body.as_ref()?.stmts.clone(),
            is_arrow: false,
            span: function.function.span,
        }),
        _ => None,
    }
}

/// The body runs as a CommonJS module with `module` and `exports` in its
/// first two parameters. It may only read globals, its own bindings, and
/// other accessors through zero-argument calls; `this` is the exports object
/// in both places, except in an arrow, where it was the bootstrap's.
fn factory_is_movable(
    factory: &Factory,
    wrapper_ids: &HashSet<Id>,
    top_ctxt: SyntaxContext,
) -> Option<()> {
    if factory.params.len() > 2
        || !factory
            .params
            .iter()
            .all(|param| matches!(param, Pat::Ident(_)))
    {
        return None;
    }
    let observes_boundary = if factory.is_arrow {
        stmts_have_function_level_special_bindings(&factory.body)
    } else {
        stmts_have_function_level_arguments_or_new_target(&factory.body)
    };
    if observes_boundary || stmts_have_function_level_return(&factory.body) {
        return None;
    }
    let mut scan = BootstrapReadScan {
        wrapper_ids,
        top_ctxt,
        reads: 0,
        accessor_calls: 0,
    };
    factory.body.visit_with(&mut scan);
    (scan.reads == scan.accessor_calls).then_some(())
}

/// Counts references to bootstrap bindings and how many of them are
/// zero-argument accessor calls.
struct BootstrapReadScan<'a> {
    wrapper_ids: &'a HashSet<Id>,
    top_ctxt: SyntaxContext,
    reads: usize,
    accessor_calls: usize,
}

impl Visit for BootstrapReadScan<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if accessor_call_target(call, self.wrapper_ids).is_some() {
            self.accessor_calls += 1;
        }
        call.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if ident.ctxt == self.top_ctxt {
            self.reads += 1;
        }
    }
}

fn accessor_call_target<'a>(call: &'a CallExpr, wrapper_ids: &HashSet<Id>) -> Option<&'a Ident> {
    if !call.args.is_empty() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    match strip_parens(callee) {
        Expr::Ident(ident) if wrapper_ids.contains(&ident.to_id()) => Some(ident),
        _ => None,
    }
}

/// Every accessor is used only as a zero-argument call, and the runtime each
/// one is built by cannot be replaced or reached by other code: its object
/// is only read through static members, the runtime member or binding is
/// installed once before the accessors and used for nothing else.
fn accessors_are_exclusive(
    analysis: &Module,
    runtimes: &[RuntimeSite],
    wrappers: &[Wrapper],
) -> Option<()> {
    let wrapper_ids: HashSet<Id> = wrappers.iter().map(|wrapper| wrapper.id.clone()).collect();
    let mut hosts: Vec<RuntimeHost> = Vec::new();
    for wrapper in wrappers {
        if let WrapperCallee::Host(host) = &wrapper.callee {
            if !hosts.contains(host) {
                hosts.push(host.clone());
            }
        }
    }
    let mut objects: HashSet<Id> = HashSet::default();
    let mut bindings: HashSet<Id> = HashSet::default();
    for host in &hosts {
        let mut sites = runtimes.iter().filter(|site| &site.host == host);
        let site = sites.next()?;
        if sites.next().is_some() {
            return None;
        }
        if wrappers.iter().any(|wrapper| {
            matches!(&wrapper.callee, WrapperCallee::Host(used) if used == host)
                && wrapper.stmt <= site.stmt
        }) {
            return None;
        }
        match host {
            RuntimeHost::Member(object, _) => objects.insert(object.clone()),
            RuntimeHost::Binding(binding) => bindings.insert(binding.clone()),
        };
    }

    let mut scan = UseScan {
        wrapper_ids: &wrapper_ids,
        objects: &objects,
        tracked: wrapper_ids
            .iter()
            .chain(&objects)
            .chain(&bindings)
            .cloned()
            .collect(),
        total: HashMap::default(),
        declared: HashMap::default(),
        accessor_calls: HashMap::default(),
        static_members: HashMap::default(),
        member_uses: HashMap::default(),
    };
    analysis.visit_with(&mut scan);

    let count = |map: &HashMap<Id, usize>, id: &Id| map.get(id).copied().unwrap_or(0);
    for id in &wrapper_ids {
        if count(&scan.declared, id) != 1
            || count(&scan.total, id) != count(&scan.accessor_calls, id) + 1
        {
            return None;
        }
    }
    for id in &objects {
        if count(&scan.declared, id) != 1
            || count(&scan.total, id) != count(&scan.static_members, id) + 1
        {
            return None;
        }
    }
    for host in &hosts {
        let users = wrappers
            .iter()
            .filter(|wrapper| matches!(&wrapper.callee, WrapperCallee::Host(used) if used == host))
            .count();
        match host {
            RuntimeHost::Member(object, prop) => {
                let installs = runtimes
                    .iter()
                    .filter(|site| &site.host == host && site.assigned)
                    .count();
                let uses = scan
                    .member_uses
                    .get(&(object.clone(), prop.clone()))
                    .copied()
                    .unwrap_or(0);
                if uses != users + installs {
                    return None;
                }
            }
            RuntimeHost::Binding(binding) => {
                if count(&scan.declared, binding) != 1 || count(&scan.total, binding) != users + 1 {
                    return None;
                }
            }
        }
    }
    Some(())
}

struct UseScan<'a> {
    wrapper_ids: &'a HashSet<Id>,
    objects: &'a HashSet<Id>,
    tracked: HashSet<Id>,
    /// Every identifier occurrence of a tracked binding.
    total: HashMap<Id, usize>,
    /// Declarators naming a tracked binding.
    declared: HashMap<Id, usize>,
    accessor_calls: HashMap<Id, usize>,
    /// `R.p` reads and writes of a runtime object, by object.
    static_members: HashMap<Id, usize>,
    /// The same, by object and property.
    member_uses: HashMap<(Id, Atom), usize>,
}

impl Visit for UseScan<'_> {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        if let Pat::Ident(name) = &decl.name {
            let id = name.id.to_id();
            if self.tracked.contains(&id) {
                *self.declared.entry(id).or_default() += 1;
            }
        }
        decl.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(ident) = accessor_call_target(call, self.wrapper_ids) {
            *self.accessor_calls.entry(ident.to_id()).or_default() += 1;
        }
        call.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if let (Expr::Ident(object), Some(prop)) = (&*member.obj, static_prop_name(&member.prop)) {
            let id = object.to_id();
            if self.objects.contains(&id) {
                *self.static_members.entry(id.clone()).or_default() += 1;
                *self.member_uses.entry((id, Atom::from(prop))).or_default() += 1;
            }
        }
        member.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        let id = ident.to_id();
        if self.tracked.contains(&id) {
            *self.total.entry(id).or_default() += 1;
        }
    }
}

/// A binding named `require` anywhere in the bootstrap would capture the
/// emitted `require(…)` calls.
fn binds_require(analysis: &Module, unresolved_mark: Mark) -> bool {
    struct Finder {
        unresolved_mark: Mark,
        found: bool,
    }
    impl Visit for Finder {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == *"require" && ident.ctxt.outer() != self.unresolved_mark {
                self.found = true;
            }
        }
    }
    let mut finder = Finder {
        unresolved_mark,
        found: false,
    };
    analysis.visit_with(&mut finder);
    finder.found
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// The bootstrap without the accessor declarations and the runtime they
/// used (its object property, assignment, or binding).
fn entry_statements(
    analysis: &Module,
    runtimes: &[RuntimeSite],
    wrappers: &[Wrapper],
) -> Vec<Stmt> {
    let mut removed_decls: HashSet<(usize, usize)> = wrappers
        .iter()
        .map(|wrapper| (wrapper.stmt, wrapper.decl))
        .collect();
    let mut used_hosts: Vec<&RuntimeHost> = Vec::new();
    for wrapper in wrappers {
        if let WrapperCallee::Host(host) = &wrapper.callee {
            used_hosts.push(host);
        }
    }
    let used_sites: Vec<&RuntimeSite> = runtimes
        .iter()
        .filter(|site| used_hosts.contains(&&site.host))
        .collect();

    let mut out = Vec::new();
    for (index, item) in analysis.body.iter().enumerate() {
        let ModuleItem::Stmt(stmt) = item else {
            continue;
        };
        let mut stmt = stmt.clone();
        for site in used_sites.iter().filter(|site| site.stmt == index) {
            match &site.host {
                RuntimeHost::Binding(binding) => {
                    if let Stmt::Decl(Decl::Var(var)) = &stmt {
                        if let Some(decl_index) = var.decls.iter().position(|decl| {
                            matches!(&decl.name, Pat::Ident(name) if name.id.to_id() == *binding)
                        }) {
                            removed_decls.insert((index, decl_index));
                        }
                    }
                }
                RuntimeHost::Member(object, prop) if site.assigned => {
                    remove_runtime_assignment(&mut stmt, object, prop);
                }
                RuntimeHost::Member(object, prop) => {
                    remove_runtime_property(&mut stmt, object, prop);
                }
            }
        }
        if let Stmt::Decl(Decl::Var(var)) = &mut stmt {
            let mut decl_index = 0;
            var.decls.retain(|_| {
                let keep = !removed_decls.contains(&(index, decl_index));
                decl_index += 1;
                keep
            });
            if var.decls.is_empty() {
                continue;
            }
        }
        if matches!(&stmt, Stmt::Empty(_)) {
            continue;
        }
        out.push(stmt);
    }
    out
}

/// Removes the require-scope object `object` from `stmts` when the code left
/// does not use it: its declaration and its runtime installs
/// (`object.d = (…) => …`) only defined values nothing reads any more.
/// `false`, with `stmts` unchanged, when a use is left or an install or the
/// declaration does more than define a value.
fn drop_unused_require_scope(stmts: &mut Vec<Stmt>, object: &Id, top_ctxt: SyntaxContext) -> bool {
    let is_object = |expr: &Expr| matches!(expr, Expr::Ident(ident) if ident.to_id() == *object);
    let is_install = |expr: &Expr| {
        let Expr::Assign(assign) = strip_parens(expr) else {
            return false;
        };
        let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
            return false;
        };
        assign.op == AssignOp::Assign
            && is_object(&member.obj)
            && static_prop_name(&member.prop).is_some()
            && is_inert_expr(&assign.right, top_ctxt)
    };
    let is_declarator =
        |decl: &VarDeclarator| matches!(&decl.name, Pat::Ident(name) if name.id.to_id() == *object);

    let mut kept_stmts = Vec::with_capacity(stmts.len());
    let mut declared = false;
    for stmt in stmts.iter() {
        match stmt {
            Stmt::Decl(Decl::Var(var)) if var.decls.iter().any(is_declarator) => {
                for decl in var.decls.iter().filter(|decl| is_declarator(decl)) {
                    if !decl.init.as_deref().is_some_and(|init| {
                        matches!(strip_parens(init), Expr::Object(_))
                            && is_inert_expr(init, top_ctxt)
                    }) {
                        return false;
                    }
                    declared = true;
                }
                let mut var = var.clone();
                var.decls.retain(|decl| !is_declarator(decl));
                if !var.decls.is_empty() {
                    kept_stmts.push(Stmt::Decl(Decl::Var(var)));
                }
            }
            Stmt::Expr(ExprStmt { span, expr }) => {
                let elements: Vec<&Expr> = match strip_parens(expr) {
                    Expr::Seq(seq) => seq.exprs.iter().map(|expr| &**expr).collect(),
                    expr => vec![expr],
                };
                if !elements.iter().any(|element| is_install(element)) {
                    kept_stmts.push(stmt.clone());
                    continue;
                }
                let mut kept: Vec<Box<Expr>> = elements
                    .into_iter()
                    .filter(|element| !is_install(element))
                    .map(|element| Box::new(element.clone()))
                    .collect();
                flush_elements(&mut kept, *span, &mut kept_stmts);
            }
            stmt => kept_stmts.push(stmt.clone()),
        }
    }
    if !declared {
        return false;
    }

    struct Mentions<'a> {
        object: &'a Id,
        found: bool,
    }
    impl Visit for Mentions<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            self.found |= ident.to_id() == *self.object;
        }
    }
    let mut mentions = Mentions {
        object,
        found: false,
    };
    kept_stmts.visit_with(&mut mentions);
    if mentions.found {
        return false;
    }
    *stmts = kept_stmts;
    true
}

fn reads_es_module_marker(stmts: &[Stmt]) -> bool {
    struct Finder {
        found: bool,
    }
    impl Visit for Finder {
        fn visit_member_prop(&mut self, prop: &MemberProp) {
            self.found |= static_prop_name(prop) == Some("__esModule");
            prop.visit_children_with(self);
        }

        fn visit_str(&mut self, value: &Str) {
            self.found |= value.value == *"__esModule";
        }
    }
    let mut finder = Finder { found: false };
    stmts.visit_with(&mut finder);
    finder.found
}

fn is_runtime_assignment(expr: &Expr, object: &Id, prop: &Atom) -> bool {
    assign_elements(expr).any(|(member, value)| {
        matches!(&*member.obj, Expr::Ident(ident) if ident.to_id() == *object)
            && static_prop_name(&member.prop).is_some_and(|name| name == prop.as_str())
            && is_cw_runtime(value)
    })
}

fn remove_runtime_assignment(stmt: &mut Stmt, object: &Id, prop: &Atom) {
    let Stmt::Expr(ExprStmt { expr, span }) = stmt else {
        return;
    };
    if let Expr::Seq(seq) = strip_parens_mut(expr) {
        seq.exprs
            .retain(|element| !is_runtime_assignment(element, object, prop));
        match seq.exprs.len() {
            0 => *stmt = Stmt::Empty(swc_core::ecma::ast::EmptyStmt { span: *span }),
            1 => {
                let only = seq.exprs.pop().expect("one element");
                *expr = only;
            }
            _ => {}
        }
    } else if is_runtime_assignment(expr, object, prop) {
        *stmt = Stmt::Empty(swc_core::ecma::ast::EmptyStmt { span: *span });
    }
}

fn strip_parens_mut(expr: &mut Expr) -> &mut Expr {
    match expr {
        Expr::Paren(paren) => strip_parens_mut(&mut paren.expr),
        expr => expr,
    }
}

fn remove_runtime_property(stmt: &mut Stmt, object: &Id, prop: &Atom) {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return;
    };
    for decl in &mut var.decls {
        if !matches!(&decl.name, Pat::Ident(name) if name.id.to_id() == *object) {
            continue;
        }
        let Some(init) = decl.init.as_deref_mut() else {
            continue;
        };
        let Expr::Object(literal) = strip_parens_mut(init) else {
            continue;
        };
        literal.props.retain(|entry| {
            let PropOrSpread::Prop(entry) = entry else {
                return true;
            };
            let Prop::KeyValue(kv) = &**entry else {
                return true;
            };
            !(prop_name_str(&kv.key).is_some_and(|key| key == prop.as_str())
                && is_cw_runtime(&kv.value))
        });
    }
}

/// Rewrites each accessor call `X()` to `require("<X's file>")`, or to the
/// binding `X` when [`bind_import_stubs`] made `X` hold the exports.
struct AccessorCallRewriter<'a> {
    targets: &'a HashMap<Id, String>,
    bound: &'a HashSet<Id>,
    from_filename: &'a str,
    require_ctxt: SyntaxContext,
}

impl VisitMut for AccessorCallRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        let Expr::Call(call) = expr else {
            return;
        };
        if !call.args.is_empty() {
            return;
        }
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Ident(ident) = strip_parens(callee) else {
            return;
        };
        let id = ident.to_id();
        if self.bound.contains(&id) {
            *expr = Expr::Ident(Ident::new(id.0, call.span, id.1));
            return;
        }
        let Some(target) = self.targets.get(&id) else {
            return;
        };
        let specifier = relative_import_specifier(self.from_filename, target);
        *expr = require_call(call.span, &specifier, self.require_ctxt);
    }
}

fn require_call(span: Span, specifier: &str, require_ctxt: SyntaxContext) -> Expr {
    Expr::Call(CallExpr {
        span,
        callee: Callee::Expr(Box::new(Expr::Ident(Ident::new(
            Atom::from("require"),
            DUMMY_SP,
            require_ctxt,
        )))),
        args: vec![Expr::Lit(Lit::Str(Str {
            span: DUMMY_SP,
            value: specifier.into(),
            raw: None,
        }))
        .into()],
        ..Default::default()
    })
}

/// Turns the import-position stub `X();` of each accessor in `settled` into
/// `var X = require("<X's file>");` and returns the accessors bound this way.
///
/// After its first call the accessor returns the same `module.exports` every
/// time, as long as the body changes it only while it loads (see
/// [`exports_settle_on_load`]); the body cannot call back into the
/// bootstrap, so that first call completes the load. A binding then holds
/// what every later call returns. That needs the stub to be the first call:
/// every top-level statement before it runs nothing a call could observe
/// ([`is_inert_stmt`]), so no function of the bootstrap has run and no
/// other call of the accessor can come first. Stubs keep that true: a body
/// cannot reach bootstrap code. A top-level comma sequence counts as one
/// statement per element and is split at the stub.
///
/// A stub's load also completes every accessor its body calls on every run
/// (`loads`, transitively), such as the module a pass-through module
/// re-exports; each of those is bound right after the stub.
fn bind_import_stubs(
    stmts: &mut Vec<Stmt>,
    settled: &HashSet<Id>,
    loads: &HashMap<Id, Vec<Id>>,
    targets: &HashMap<Id, String>,
    require_ctxt: SyntaxContext,
    top_ctxt: SyntaxContext,
) -> HashSet<Id> {
    let wrapper_ids: HashSet<Id> = targets.keys().cloned().collect();
    let binding = |id: &Id, span: Span| {
        let specifier = relative_import_specifier(ENTRY_FILENAME, &targets[id]);
        Stmt::Decl(Decl::Var(Box::new(VarDecl {
            span,
            kind: swc_core::ecma::ast::VarDeclKind::Var,
            decls: vec![VarDeclarator {
                span,
                name: Pat::Ident(Ident::new(id.0.clone(), DUMMY_SP, id.1).into()),
                init: Some(Box::new(require_call(span, &specifier, require_ctxt))),
                definite: false,
            }],
            ..Default::default()
        })))
    };
    let mut loaded: HashSet<Id> = HashSet::default();
    let mut bound: HashSet<Id> = HashSet::default();
    let mut out = Vec::with_capacity(stmts.len());
    let mut inert_so_far = true;
    for stmt in std::mem::take(stmts) {
        if !inert_so_far {
            out.push(stmt);
            continue;
        }
        let Stmt::Expr(ExprStmt { span, expr }) = stmt else {
            inert_so_far = is_inert_stmt(&stmt, top_ctxt);
            out.push(stmt);
            continue;
        };
        let elements: Vec<Box<Expr>> = match *expr {
            Expr::Seq(seq) => seq.exprs,
            expr => vec![Box::new(expr)],
        };
        let mut pending: Vec<Box<Expr>> = Vec::new();
        for element in elements {
            if !inert_so_far {
                pending.push(element);
                continue;
            }
            let Some(id) = stub_target(&element, &wrapper_ids) else {
                inert_so_far = is_inert_expr(&element, top_ctxt);
                pending.push(element);
                continue;
            };
            if loaded.contains(&id) {
                // Already loaded: the call only reads the exports.
                pending.push(element);
                continue;
            }
            let mut completed = vec![id.clone()];
            let mut index = 0;
            while let Some(current) = completed.get(index).cloned() {
                index += 1;
                for next in loads.get(&current).into_iter().flatten() {
                    if !completed.contains(next) && !loaded.contains(next) {
                        completed.push(next.clone());
                    }
                }
            }
            flush_elements(&mut pending, span, &mut out);
            let call_span = element.span();
            if settled.contains(&id) {
                out.push(binding(&id, call_span));
                bound.insert(id.clone());
            } else {
                out.push(Stmt::Expr(ExprStmt {
                    span: call_span,
                    expr: element,
                }));
            }
            for other in &completed[1..] {
                if settled.contains(other) {
                    out.push(binding(other, call_span));
                    bound.insert(other.clone());
                }
            }
            loaded.extend(completed);
        }
        flush_elements(&mut pending, span, &mut out);
    }
    *stmts = out;
    bound
}

/// The accessors a wrapped body calls on every run that returns: zero-argument
/// calls of `wrapper_ids` on the always-evaluated part of its top-level
/// expression statements and declarations (not inside a function, a branch,
/// or the right of a short-circuit). The body has no top-level `return`, so
/// every top-level statement runs unless an earlier one throws, and then the
/// stub throws too.
fn unconditional_loads(body: &[Stmt], wrapper_ids: &HashSet<Id>) -> Vec<Id> {
    fn walk(expr: &Expr, wrapper_ids: &HashSet<Id>, out: &mut Vec<Id>) {
        match expr {
            Expr::Paren(paren) => walk(&paren.expr, wrapper_ids, out),
            Expr::Call(call) => {
                if let Some(ident) = accessor_call_target(call, wrapper_ids) {
                    out.push(ident.to_id());
                    return;
                }
                if let Callee::Expr(callee) = &call.callee {
                    walk(callee, wrapper_ids, out);
                }
                for arg in &call.args {
                    walk(&arg.expr, wrapper_ids, out);
                }
            }
            Expr::New(new) => {
                walk(&new.callee, wrapper_ids, out);
                for arg in new.args.iter().flatten() {
                    walk(&arg.expr, wrapper_ids, out);
                }
            }
            Expr::Member(member) => {
                walk(&member.obj, wrapper_ids, out);
                if let MemberProp::Computed(computed) = &member.prop {
                    walk(&computed.expr, wrapper_ids, out);
                }
            }
            Expr::Assign(assign) => {
                if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left {
                    walk(&member.obj, wrapper_ids, out);
                }
                walk(&assign.right, wrapper_ids, out);
            }
            Expr::Seq(seq) => {
                for expr in &seq.exprs {
                    walk(expr, wrapper_ids, out);
                }
            }
            Expr::Bin(bin) => {
                walk(&bin.left, wrapper_ids, out);
                if !matches!(
                    bin.op,
                    swc_core::ecma::ast::BinaryOp::LogicalAnd
                        | swc_core::ecma::ast::BinaryOp::LogicalOr
                        | swc_core::ecma::ast::BinaryOp::NullishCoalescing
                ) {
                    walk(&bin.right, wrapper_ids, out);
                }
            }
            Expr::Cond(cond) => walk(&cond.test, wrapper_ids, out),
            Expr::Unary(unary) => walk(&unary.arg, wrapper_ids, out),
            Expr::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    walk(&elem.expr, wrapper_ids, out);
                }
            }
            Expr::Object(object) => {
                for prop in &object.props {
                    match prop {
                        PropOrSpread::Spread(spread) => walk(&spread.expr, wrapper_ids, out),
                        PropOrSpread::Prop(prop) => {
                            if let Prop::KeyValue(kv) = &**prop {
                                if let PropName::Computed(key) = &kv.key {
                                    walk(&key.expr, wrapper_ids, out);
                                }
                                walk(&kv.value, wrapper_ids, out);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for stmt in body {
        match stmt {
            Stmt::Expr(ExprStmt { expr, .. }) => walk(expr, wrapper_ids, &mut out),
            Stmt::Decl(Decl::Var(var)) => {
                for decl in &var.decls {
                    if let Some(init) = &decl.init {
                        walk(init, wrapper_ids, &mut out);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn stub_target(element: &Expr, wrapper_ids: &HashSet<Id>) -> Option<Id> {
    let Expr::Call(call) = strip_parens(element) else {
        return None;
    };
    accessor_call_target(call, wrapper_ids).map(Ident::to_id)
}

fn flush_elements(pending: &mut Vec<Box<Expr>>, span: Span, out: &mut Vec<Stmt>) {
    let expr = match pending.len() {
        0 => return,
        1 => pending.pop().expect("one element"),
        _ => Box::new(Expr::Seq(swc_core::ecma::ast::SeqExpr {
            span,
            exprs: std::mem::take(pending),
        })),
    };
    out.push(Stmt::Expr(ExprStmt { span, expr }));
}

/// A top-level statement that runs no code of the bootstrap and no getter or
/// setter: a directive, a function declaration, or a declaration or
/// expression built from [`is_inert_expr`] values.
fn is_inert_stmt(stmt: &Stmt, top_ctxt: SyntaxContext) -> bool {
    match stmt {
        Stmt::Empty(_) | Stmt::Decl(Decl::Fn(_)) => true,
        Stmt::Decl(Decl::Var(var)) => var.decls.iter().all(|decl| {
            matches!(decl.name, Pat::Ident(_))
                && decl
                    .init
                    .as_deref()
                    .is_none_or(|init| is_inert_expr(init, top_ctxt))
        }),
        Stmt::Expr(ExprStmt { expr, .. }) => match strip_parens(expr) {
            Expr::Seq(seq) => seq.exprs.iter().all(|expr| is_inert_expr(expr, top_ctxt)),
            expr => is_inert_expr(expr, top_ctxt),
        },
        _ => false,
    }
}

/// An expression whose evaluation calls nothing: a literal, a name, a
/// function, an object or array literal of such values with plain keys, or an
/// assignment of one to a bootstrap binding or to a plain key of one's
/// property (the runtime's `R.d = (…) => …`). Only a write to code outside
/// the bootstrap (a global's setter) could hand a bootstrap function out
/// before the stub; `__proto__` is a setter of every object.
fn is_inert_expr(expr: &Expr, top_ctxt: SyntaxContext) -> bool {
    let inert = |expr: &Expr| is_inert_expr(expr, top_ctxt);
    match strip_parens(expr) {
        Expr::Lit(_) | Expr::Ident(_) | Expr::Fn(_) | Expr::Arrow(_) => true,
        Expr::Unary(unary) => {
            unary.op != swc_core::ecma::ast::UnaryOp::Delete
                && matches!(strip_parens(&unary.arg), Expr::Lit(_))
        }
        Expr::Array(array) => array.elems.iter().all(|elem| {
            elem.as_ref()
                .is_none_or(|elem| elem.spread.is_none() && inert(&elem.expr))
        }),
        Expr::Object(object) => object.props.iter().all(|prop| match prop {
            PropOrSpread::Spread(_) => false,
            PropOrSpread::Prop(prop) => match &**prop {
                Prop::Shorthand(_) => true,
                Prop::KeyValue(kv) => is_plain_key(&kv.key) && inert(&kv.value),
                Prop::Method(method) => is_plain_key(&method.key),
                _ => false,
            },
        }),
        Expr::Assign(assign) if assign.op == AssignOp::Assign => {
            inert(&assign.right)
                && match &assign.left {
                    AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                        binding.id.ctxt == top_ctxt
                    }
                    AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                        matches!(&*member.obj, Expr::Ident(object) if object.ctxt == top_ctxt)
                            && static_prop_name(&member.prop)
                                .is_some_and(|name| name != "__proto__")
                    }
                    _ => false,
                }
        }
        _ => false,
    }
}

fn is_plain_key(key: &PropName) -> bool {
    match key {
        PropName::Computed(_) => false,
        key => prop_name_str(key).is_none_or(|name| name != "__proto__"),
    }
}

/// Whether the body touches its module object only as `module.exports` and
/// only while it runs: no function or class in it reads the module object, so
/// nothing can replace `module.exports` once the body has returned.
fn exports_settle_on_load(factory: &Factory) -> bool {
    let Some(Pat::Ident(module)) = factory.params.first() else {
        return true;
    };
    let mut scan = ModuleObjectScan {
        module: module.id.to_id(),
        depth: 0,
        settles: true,
    };
    factory.body.visit_with(&mut scan);
    scan.settles
}

struct ModuleObjectScan {
    module: Id,
    depth: usize,
    settles: bool,
}

impl ModuleObjectScan {
    fn nested(&mut self, visit: impl FnOnce(&mut Self)) {
        self.depth += 1;
        visit(self);
        self.depth -= 1;
    }
}

impl Visit for ModuleObjectScan {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if let Expr::Ident(object) = &*member.obj {
            if object.to_id() == self.module {
                if self.depth > 0 || static_prop_name(&member.prop) != Some("exports") {
                    self.settles = false;
                }
                if let MemberProp::Computed(computed) = &member.prop {
                    computed.visit_with(self);
                }
                return;
            }
        }
        member.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        // Any use other than a `module.exports` member.
        if ident.to_id() == self.module {
            self.settles = false;
        }
    }

    fn visit_function(&mut self, function: &Function) {
        self.nested(|scan| function.visit_children_with(scan));
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.nested(|scan| arrow.visit_children_with(scan));
    }

    fn visit_class(&mut self, class: &swc_core::ecma::ast::Class) {
        self.nested(|scan| class.visit_children_with(scan));
    }

    fn visit_getter_prop(&mut self, prop: &swc_core::ecma::ast::GetterProp) {
        self.nested(|scan| prop.visit_children_with(scan));
    }

    fn visit_setter_prop(&mut self, prop: &swc_core::ecma::ast::SetterProp) {
        self.nested(|scan| prop.visit_children_with(scan));
    }
}

struct ClearContexts;

impl VisitMut for ClearContexts {
    fn visit_mut_syntax_context(&mut self, ctxt: &mut SyntaxContext) {
        *ctxt = SyntaxContext::empty();
    }
}

fn is_use_strict(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Expr(ExprStmt { expr, .. })
            if matches!(&**expr, Expr::Lit(Lit::Str(value)) if value.value == *"use strict")
    )
}

/// Whether the directive prologue of `stmts` has `"use strict"`.
fn has_use_strict_directive(stmts: &[Stmt]) -> bool {
    stmts
        .iter()
        .take_while(|stmt| {
            matches!(stmt, Stmt::Expr(ExprStmt { expr, .. }) if matches!(&**expr, Expr::Lit(Lit::Str(_))))
        })
        .any(is_use_strict)
}

fn use_strict_directive() -> Stmt {
    Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Lit(Lit::Str(Str {
            span: DUMMY_SP,
            value: "use strict".into(),
            raw: None,
        }))),
    })
}

/// The bootstrap's own prologue or the file's makes the wrapped bodies strict.
fn bootstrap_is_strict(
    module_items: &[ModuleItem],
    bundle_idx: usize,
    bootstrap_body: &FunctionBody,
) -> bool {
    if has_use_strict_directive(&bootstrap_body.stmts) {
        return true;
    }
    let prologue: Vec<Stmt> = module_items[..bundle_idx]
        .iter()
        .map_while(|item| match item {
            ModuleItem::Stmt(stmt @ Stmt::Expr(ExprStmt { expr, .. }))
                if matches!(&**expr, Expr::Lit(Lit::Str(_))) =>
            {
                Some(stmt.clone())
            }
            _ => None,
        })
        .collect();
    prologue.len() == bundle_idx && has_use_strict_directive(&prologue)
}

/// One filename per wrapper: the path in webpack's `// MODULE: <path>`
/// comment right before a single-declarator declaration, otherwise
/// `module-cw-<n>.js` by declaration order. Names are unique and never
/// `entry.js`.
fn wrapper_filenames(cm: &SourceMap, stmts: &[&Stmt], wrappers: &[Wrapper]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::default();
    seen.insert(crate::driver::filename_case_key(ENTRY_FILENAME));
    wrappers
        .iter()
        .enumerate()
        .map(|(index, wrapper)| {
            let hint = match stmts[wrapper.stmt] {
                Stmt::Decl(Decl::Var(var)) if var.decls.len() == 1 => {
                    module_comment_path(cm, var.span)
                }
                _ => None,
            };
            let filename = match hint {
                Some(path) => webpack_module_filename(&path),
                None => webpack_module_filename(&format!("cw-{index}")),
            };
            dedup_filename(
                &filename,
                &mut seen,
                FilenameDedupStyle::PathAware {
                    fallback_stem: "module",
                },
            )
        })
        .collect()
}

/// The path of a `// MODULE: <path>` line comment that is the last thing
/// before `span` in the input.
fn module_comment_path(cm: &SourceMap, span: Span) -> Option<String> {
    if span.lo.0 == 0 {
        return None;
    }
    let location = cm.lookup_byte_offset(span.lo);
    let before = location.sf.src.get(..location.pos.0 as usize)?;
    let line = before.trim_end().rsplit('\n').next()?.trim();
    let path = line.strip_prefix("// MODULE: ")?.trim();
    (!path.is_empty() && !path.contains(char::is_whitespace)).then(|| path.to_string())
}

// ---------------------------------------------------------------------------
// Wrapped modules beside a module table
// ---------------------------------------------------------------------------

/// The id the module at `index` of a [`TableWrapperSplit`] gets among the
/// table's modules. A webpack module id never has this form; the table path
/// still checks that no table module uses it.
pub(super) fn table_wrapper_id(index: usize) -> String {
    format!("cw:{index}")
}

/// The `.cw` wrapped modules of a bootstrap that also has a module table, as
/// extra factories of that table (see [`split_table_wrappers`]).
pub(super) struct TableWrapperSplit {
    /// The bootstrap without what moved to the factories, contexts cleared.
    pub(super) bootstrap: FunctionBody,
    /// One factory per wrapped module in source order, then one per
    /// namespace object, with parameters `(module, exports, <require_sym>)`;
    /// the one at `index` has the id [`table_wrapper_id`] gives.
    pub(super) factories: Vec<TranslatedWebpackFactory>,
    /// The require function's name in `bootstrap`.
    pub(super) require_sym: Atom,
    /// The table modules whose loaded exports a factory now reads through
    /// `<require_sym>(<id>)` instead of a binding of the bootstrap: each of
    /// them must replace `module.exports` only while it loads.
    pub(super) loaded_module_ids: Vec<Expr>,
}

/// A wrapped module of a bootstrap with a module table.
struct TableWrapped {
    /// The accessor binding; `None` for a wrapper called where it is built,
    /// `R.cw(function () { … })()` (Terser inlining a one-use accessor).
    id: Option<Id>,
    stmt: usize,
    /// For an accessor binding, the index of its declarator in `stmt`.
    decl: usize,
    /// The wrapped function's span, which identifies it.
    function_span: Span,
}

/// A binding of the wrapper scope a wrapped body may read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeBindingKind {
    /// `X = () => R(<id>)`: a table module's accessor.
    Accessor,
    /// `X = {}` that `R.r(X)` and `R.d(X, { … })` fill: a namespace object.
    Namespace,
    /// `X = R(<id>)`: a table module's exports, loaded in place.
    Loaded,
}

struct ScopeBinding {
    id: Id,
    kind: ScopeBindingKind,
    stmt: usize,
    decl: usize,
    /// The module id literal of an accessor or a loaded binding.
    module_id: Option<Expr>,
}

/// Turn the `.cw` wrapped modules of a bootstrap that also has a module table
/// into factories of that table.
///
/// The runtime is installed on the require function `R` (`R.cw = …`). A
/// wrapped body may read `R` itself (`R.d(exports, …)`, `R.e(id)`, `R(id)`),
/// the accessors of other wrapped modules, and what webpack declares for the
/// table modules beside them: it is the factory `(module, exports, R)` with
/// `R` closed over, called the way `R` calls a table factory,
/// `fn.call(module.exports, module, module.exports, R)`. Each wrapped body
/// becomes such a factory, and the bindings around it become loads:
///
/// - a zero-argument accessor call, of a wrapped module or of a table module
///   (`var X = () => R(<id>)`), becomes `R(<id>)`. Both load the module once,
///   on the first call, and return its `module.exports`;
/// - a namespace object (`var N = {}` filled by `R.r(N), R.d(N, { a: () =>
///   X().a })`) that a moved body reads, or one among the declarations
///   that entry code reads, becomes a factory of its own holding those
///   calls, and every read of it `R(<its id>)`: one object, defined before
///   anything reads it. One nothing reads goes with its calls;
/// - a body's read of a table module loaded in place (`var X = R(<id>)`)
///   becomes `R(<id>)`, which returns the same exports once the load has
///   run, as long as the module keeps its `module.exports`
///   ([`TableWrapperSplit::loaded_module_ids`]);
/// - an import-position stub `X();` becomes `var X = R(<id>)` that later
///   entry calls read, as in a bootstrap without a table
///   ([`table_import_stubs`]).
///
/// The reads keep their values because nothing can run a moved body, an
/// accessor, or a namespace getter before every binding they read is
/// initialized. webpack declares the accessors and namespace objects first,
/// with only runtime helper calls in between; then the loads of table
/// modules; then the code that calls accessors. So outside wrapped bodies,
/// which run only through a call: nothing before the first declaration
/// mentions a binding declared from there on (so neither the runtime nor a
/// table module can reach an accessor), the statements from
/// there to the last declaration only define values and call `R`'s members,
/// the statements after them up to the last load a body reads mention only
/// their own bindings, `R`, globals, and table module accessors (a table
/// module cannot reach the bootstrap's bindings), and an accessor or a moved
/// namespace object is read in place only after that.
///
/// `R` gets a name no other identifier of the bootstrap spells, so no
/// binding can capture the inserted calls. The wrappers sit at the top level
/// of the bootstrap, or of the startup IIFE that ends it
/// (`(() => { "use strict"; … })()`), which runs once.
///
/// `None` leaves the bootstrap to the table path as it is: an accessor of a
/// wrapped module is used other than as a zero-argument call (a table
/// module's accessor used that way stays entry code, unless a body reads
/// it), `R.cw` is used for anything but building wrappers or `R` is written,
/// one of the orders above does not hold, a body reads another binding
/// around it or observes its function boundary, or the bootstrap has a
/// `with` statement or a direct `eval`.
pub(super) fn split_table_wrappers(
    bootstrap_body: &FunctionBody,
    cm: &SourceMap,
) -> Option<TableWrapperSplit> {
    if !bootstrap_body.stmts.iter().any(stmt_holds_runtime) {
        return None;
    }
    let span = tracing::info_span!("webpack5: split_table_cw");
    let _enter = span.enter();

    let mut analysis = Module {
        span: DUMMY_SP,
        body: bootstrap_body
            .stmts
            .iter()
            .cloned()
            .map(ModuleItem::Stmt)
            .collect(),
        shebang: None,
    };
    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    analysis.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
    let top_ctxt = SyntaxContext::empty().apply_mark(top_level_mark);
    if node_has_dynamic_scope_construct(&analysis) {
        return None;
    }
    let mut bootstrap: Vec<Stmt> = analysis
        .body
        .into_iter()
        .filter_map(|item| match item {
            ModuleItem::Stmt(stmt) => Some(stmt),
            ModuleItem::ModuleDecl(_) => None,
        })
        .collect();

    let (require, prop, installed_at) = {
        let top: Vec<&Stmt> = bootstrap.iter().collect();
        let runtimes = collect_runtime_sites(&top, top_ctxt);
        let [site] = runtimes.as_slice() else {
            return None;
        };
        let RuntimeHost::Member(require, prop) = &site.host else {
            return None;
        };
        let require_is_function = top.iter().any(|stmt| {
            matches!(stmt, Stmt::Decl(Decl::Fn(function)) if function.ident.to_id() == *require)
        });
        if !site.assigned || !require_is_function {
            return None;
        }
        (require.clone(), prop.clone(), site.stmt)
    };

    let scope = if scope_builds_wrappers(&bootstrap, &require, &prop) {
        WrapperScope::Top
    } else {
        let scope = startup_iife_scope(&bootstrap)?;
        if !scope_builds_wrappers(scope_stmts(&bootstrap, &scope)?, &require, &prop) {
            return None;
        }
        scope
    };
    let scope_body = scope_stmts(&bootstrap, &scope)?;
    let stmts: Vec<&Stmt> = scope_body.iter().collect();
    // In the startup IIFE every statement runs after the runtime is installed.
    let installed_at = match scope {
        WrapperScope::Top => Some(installed_at),
        WrapperScope::Iife { .. } => None,
    };

    // The wrapped modules and the bindings beside them.
    let mut wrapped: Vec<TableWrapped> = Vec::new();
    let mut bindings: Vec<ScopeBinding> = Vec::new();
    for (index, stmt) in stmts.iter().enumerate() {
        if let Stmt::Decl(Decl::Var(var)) = stmt {
            for (decl_index, decl) in var.decls.iter().enumerate() {
                let Some(init) = decl.init.as_deref() else {
                    continue;
                };
                let Pat::Ident(name) = &decl.name else {
                    if is_wrapper_build(Some(init), &require, &prop) {
                        return None;
                    }
                    continue;
                };
                let id = name.id.to_id();
                if is_wrapper_build(Some(init), &require, &prop) {
                    if installed_at.is_some_and(|at| index <= at) {
                        return None;
                    }
                    let function = declarator_wrapped_function(var, decl_index)?;
                    wrapped.push(TableWrapped {
                        id: Some(id),
                        stmt: index,
                        decl: decl_index,
                        function_span: function_span(function)?,
                    });
                    continue;
                }
                let (kind, module_id) = if let Some(module_id) =
                    table_accessor_module_id(init, &require)
                {
                    (ScopeBindingKind::Accessor, Some(module_id))
                } else if let Some(module_id) = table_load_module_id(init, &require) {
                    (ScopeBindingKind::Loaded, Some(module_id))
                } else if matches!(strip_parens(init), Expr::Object(object) if object.props.is_empty())
                {
                    (ScopeBindingKind::Namespace, None)
                } else {
                    continue;
                };
                bindings.push(ScopeBinding {
                    id,
                    kind,
                    stmt: index,
                    decl: decl_index,
                    module_id,
                });
            }
        }
        for value in top_level_values(stmt) {
            let Some(function) = inline_called_wrapper(value, &require, &prop) else {
                continue;
            };
            if installed_at.is_some_and(|at| index <= at) {
                return None;
            }
            wrapped.push(TableWrapped {
                id: None,
                stmt: index,
                decl: 0,
                function_span: function_span(function)?,
            });
        }
    }
    if wrapped.is_empty() {
        return None;
    }
    wrapped.sort_by_key(|module| module.function_span.lo);

    let wrapper_ids: Vec<Id> = wrapped
        .iter()
        .filter_map(|module| module.id.clone())
        .collect();
    let namespace_ids: HashSet<Id> = bindings
        .iter()
        .filter(|binding| binding.kind == ScopeBindingKind::Namespace)
        .map(|binding| binding.id.clone())
        .collect();
    let tracked: HashSet<Id> = wrapper_ids
        .iter()
        .cloned()
        .chain(bindings.iter().map(|binding| binding.id.clone()))
        .collect();
    let lazy: HashSet<Span> = wrapped
        .iter()
        .filter(|module| module.id.is_some())
        .map(|module| module.function_span)
        .collect();
    let inline: HashSet<Span> = wrapped
        .iter()
        .filter(|module| module.id.is_none())
        .map(|module| module.function_span)
        .collect();
    let mut scan = TableUseScan {
        tracked: &tracked,
        namespaces: &namespace_ids,
        require: &require,
        prop: &prop,
        lazy: &lazy,
        inline: &inline,
        scope_body: match &scope {
            WrapperScope::Top => None,
            WrapperScope::Iife { body_span, .. } => Some(*body_span),
        },
        stmt: 0,
        lazy_depth: 0,
        function_depth: 0,
        uses: HashMap::default(),
        declared: HashMap::default(),
        calls: HashMap::default(),
        written: HashSet::default(),
        defines: HashMap::default(),
        nested_defines: HashSet::default(),
        first_use: HashMap::default(),
        first_eager_use: HashMap::default(),
        host_uses: 0,
        require_functions: 0,
        require_bound: false,
    };
    match scope {
        WrapperScope::Top => {
            for (index, stmt) in bootstrap.iter().enumerate() {
                scan.stmt = index;
                stmt.visit_with(&mut scan);
            }
        }
        WrapperScope::Iife { .. } => bootstrap.visit_with(&mut scan),
    }
    let count = |map: &HashMap<Id, usize>, id: &Id| map.get(id).copied().unwrap_or(0);
    let declared_once = |id: &Id| count(&scan.declared, id) == 1 && !scan.written.contains(id);
    let exclusive = |id: &Id| declared_once(id) && count(&scan.uses, id) == count(&scan.calls, id);
    if !wrapper_ids.iter().all(exclusive)
        || scan.host_uses != wrapped.len() + 1
        || scan.require_functions != 1
        || scan.require_bound
    {
        return None;
    }

    // Each namespace object's `R.r(N)` / `R.d(N, …)` calls, which must all
    // be top-level expressions of the scope.
    let mut define_sites: HashMap<Id, Vec<(usize, usize)>> = HashMap::default();
    for (index, stmt) in stmts.iter().enumerate() {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            continue;
        };
        let elements: Vec<&Expr> = match strip_parens(expr) {
            Expr::Seq(seq) => seq.exprs.iter().map(|expr| &**expr).collect(),
            expr => vec![expr],
        };
        for (element, value) in elements.into_iter().enumerate() {
            if let Some(id) = namespace_define_target(value, &require, &namespace_ids) {
                define_sites.entry(id).or_default().push((index, element));
            }
        }
    }
    bindings.retain(|binding| match binding.kind {
        ScopeBindingKind::Accessor => exclusive(&binding.id),
        ScopeBindingKind::Loaded => declared_once(&binding.id),
        ScopeBindingKind::Namespace => {
            declared_once(&binding.id)
                && count(&scan.defines, &binding.id) > 0
                && !scan.nested_defines.contains(&binding.id)
                && define_sites.get(&binding.id).map_or(0, Vec::len)
                    == count(&scan.defines, &binding.id)
        }
    });
    let binding_kind = |id: &Id| {
        bindings
            .iter()
            .find(|binding| binding.id == *id)
            .map(|binding| binding.kind)
    };
    let callable: HashSet<Id> = wrapper_ids
        .iter()
        .cloned()
        .chain(
            bindings
                .iter()
                .filter(|binding| binding.kind == ScopeBindingKind::Accessor)
                .map(|binding| binding.id.clone()),
        )
        .collect();

    // What the moved code reads: each wrapped body, then the getters of each
    // namespace object that moves.
    let mut needed: Vec<Id> = Vec::new();
    let mut inline_reads: Vec<bool> = Vec::with_capacity(wrapped.len());
    for module in &wrapped {
        let function = wrapped_function(stmts[module.stmt], module, &require, &prop)?;
        let reads = table_factory_reads(function, &callable, &require, unresolved_mark)?;
        inline_reads.push(module.id.is_none() && !reads.is_empty());
        needed.extend(reads);
    }
    let namespace_reads = |id: &Id| -> Option<Vec<Id>> {
        let mut reads = Vec::new();
        for &(stmt, element) in &define_sites[id] {
            let Expr::Call(call) = strip_parens(top_level_element(stmts[stmt], element)?) else {
                return None;
            };
            for arg in &call.args[1..] {
                reads.extend(expr_outer_reads(
                    &arg.expr,
                    &callable,
                    &require,
                    unresolved_mark,
                )?);
            }
        }
        Some(reads)
    };
    let mut namespaces: Vec<Id> = Vec::new();
    let mut loaded: Vec<Id> = Vec::new();
    let mut index = 0;
    while let Some(id) = needed.get(index).cloned() {
        index += 1;
        match binding_kind(&id) {
            Some(ScopeBindingKind::Accessor) => {}
            Some(ScopeBindingKind::Loaded) => {
                if !loaded.contains(&id) {
                    loaded.push(id);
                }
            }
            Some(ScopeBindingKind::Namespace) => {
                if !namespaces.contains(&id) {
                    needed.extend(namespace_reads(&id)?);
                    namespaces.push(id);
                }
            }
            None => return None,
        }
    }

    // The orders that keep every read's value (see above).
    let binding_stmt = |id: &Id| {
        bindings
            .iter()
            .find(|binding| binding.id == *id)
            .map(|binding| binding.stmt)
    };
    let namespace_stmts = |id: &Id| {
        binding_stmt(id)
            .into_iter()
            .chain(define_sites[id].iter().map(|(stmt, _)| *stmt))
            .collect::<Vec<_>>()
    };
    let definitions: Vec<usize> = wrapped
        .iter()
        .filter(|module| module.id.is_some())
        .map(|module| module.stmt)
        .chain(callable.iter().filter_map(binding_stmt))
        .chain(namespaces.iter().flat_map(namespace_stmts))
        .collect();
    let first_definition = definitions.iter().min().copied();
    let last_definition = definitions.iter().max().copied();
    let in_definitions = |stmt: usize| {
        first_definition.is_some_and(|first| first <= stmt)
            && last_definition.is_some_and(|last| stmt <= last)
    };
    // Mentioned from the first declaration on, read in place only after the
    // last.
    let defined_in_time = |id: &Id| {
        first_definition
            .is_none_or(|first| scan.first_use.get(id).is_none_or(|stmt| *stmt >= first))
            && last_definition
                .is_none_or(|last| scan.first_eager_use.get(id).is_none_or(|stmt| *stmt > last))
    };
    if !callable.iter().chain(&namespaces).all(defined_in_time) {
        return None;
    }
    if let (Some(first), Some(last)) = (first_definition, last_definition) {
        if !stmts[first..=last]
            .iter()
            .all(|stmt| is_definition_stmt(stmt, &require, top_ctxt))
        {
            return None;
        }
        // Nor can the code before the declarations (the runtime, the module
        // table) reach what is declared from there on, such as an entry
        // function that calls an accessor.
        let mut later = DeclaredIds::default();
        for stmt in &stmts[first..] {
            stmt.visit_with(&mut later);
        }
        if stmts_mention(&stmts[..first], &later.0) {
            return None;
        }
    }
    // The other namespace objects among the declarations move too, when
    // their getters read only what moved code may: entry code then reads
    // a module whose runtime calls the pipeline recovers. One nothing reads
    // only ever has its runtime calls run on it, and goes with them.
    let mut unread: Vec<Id> = Vec::new();
    let mut optional: Vec<(Id, Vec<Id>)> = Vec::new();
    for binding in &bindings {
        if binding.kind != ScopeBindingKind::Namespace
            || namespaces.contains(&binding.id)
            || !namespace_stmts(&binding.id).into_iter().all(in_definitions)
        {
            continue;
        }
        if count(&scan.uses, &binding.id) == 0 {
            unread.push(binding.id.clone());
        } else if defined_in_time(&binding.id) {
            if let Some(reads) = namespace_reads(&binding.id) {
                optional.push((binding.id.clone(), reads));
            }
        }
    }
    loop {
        let before = optional.len();
        let liftable: Vec<Id> = optional.iter().map(|(id, _)| id.clone()).collect();
        optional.retain(|(_, reads)| {
            reads.iter().all(|read| match binding_kind(read) {
                Some(ScopeBindingKind::Loaded) => true,
                Some(ScopeBindingKind::Namespace) => {
                    namespaces.contains(read) || liftable.contains(read)
                }
                _ => false,
            })
        });
        if optional.len() == before {
            break;
        }
    }
    for (id, reads) in optional {
        for read in reads {
            if binding_kind(&read) == Some(ScopeBindingKind::Loaded) && !loaded.contains(&read) {
                loaded.push(read);
            }
        }
        namespaces.push(id);
    }

    let last_load = loaded.iter().filter_map(binding_stmt).max();
    let settled = match (last_definition, last_load) {
        (Some(definition), Some(load)) if load > definition => {
            let table_accessors: HashSet<Id> = bindings
                .iter()
                .filter(|binding| binding.kind == ScopeBindingKind::Accessor)
                .map(|binding| binding.id.clone())
                .collect();
            if !stmts_read_only_their_own(
                &stmts[definition + 1..=load],
                &table_accessors,
                &require,
                unresolved_mark,
            ) {
                return None;
            }
            Some(load)
        }
        (definition, load) => definition.max(load),
    };
    for (module, reads) in wrapped.iter().zip(&inline_reads) {
        if *reads && settled.is_some_and(|settled| module.stmt <= settled) {
            return None;
        }
    }

    // The import-position stubs `X();` that become `var X = R("<id>")`, as in
    // a bootstrap without a table (see `bind_import_stubs`). Before the last
    // declaration or load nothing can call an accessor, so the scan starts
    // after it; table module loads cannot call one either.
    let stub_bindings = {
        let table_loads: HashSet<Id> = bindings
            .iter()
            .filter(|binding| binding.kind == ScopeBindingKind::Accessor)
            .map(|binding| binding.id.clone())
            .collect();
        let from = match (settled, installed_at) {
            (Some(settled), _) => settled + 1,
            (None, Some(installed)) => installed + 1,
            (None, None) => 0,
        };
        let entry = &stmts[from.min(stmts.len())..];
        let observes_interop = entry.iter().any(|stmt| {
            reads_es_module_marker(std::slice::from_ref(*stmt))
                || stmt_uses_member(stmt, &require, "n")
        });
        if observes_interop {
            Vec::new()
        } else {
            let mut settles = Vec::with_capacity(wrapped.len());
            let mut loads = Vec::with_capacity(wrapped.len());
            let wrapper_set: HashSet<Id> = wrapper_ids.iter().cloned().collect();
            for module in &wrapped {
                let factory = function_factory(wrapped_function(
                    stmts[module.stmt],
                    module,
                    &require,
                    &prop,
                )?)?;
                settles.push(module.id.is_some() && exports_settle_on_load(&factory));
                loads.push(
                    unconditional_loads(&factory.body, &wrapper_set)
                        .into_iter()
                        .filter_map(|id| {
                            wrapped
                                .iter()
                                .position(|other| other.id.as_ref() == Some(&id))
                        })
                        .collect::<Vec<_>>(),
                );
            }
            table_import_stubs(
                entry,
                from,
                &wrapped,
                &settles,
                &loads,
                &table_loads,
                &require,
                top_ctxt,
            )
        }
    };

    // A name only `R` spells: its own when no other identifier spells it.
    let mut spellings = ForeignSpellings {
        id: &require,
        syms: HashSet::default(),
    };
    bootstrap.visit_with(&mut spellings);
    let require_sym = if spellings.syms.contains(&require.0) {
        (0..)
            .map(|suffix| match suffix {
                0 => Atom::from("__webpack_require__"),
                n => Atom::from(format!("__webpack_require___{n}")),
            })
            .find(|candidate| !spellings.syms.contains(candidate))?
    } else {
        require.0.clone()
    };

    let mut filenames: Vec<String> = wrapped
        .iter()
        .enumerate()
        .map(|(index, module)| table_wrapper_filename(cm, stmts[module.stmt], module, index))
        .collect();
    filenames.extend(
        (0..namespaces.len())
            .map(|index| webpack_module_filename(&format!("cw-namespace-{index}"))),
    );
    let strict = has_use_strict_directive(&bootstrap_body.stmts)
        || matches!(scope, WrapperScope::Iife { .. }) && has_use_strict_directive(scope_body);
    let removed: HashSet<(usize, usize)> = wrapped
        .iter()
        .filter(|module| module.id.is_some())
        .map(|module| (module.stmt, module.decl))
        .chain(
            bindings
                .iter()
                .filter(|binding| {
                    binding.kind == ScopeBindingKind::Accessor
                        || namespaces.contains(&binding.id)
                        || unread.contains(&binding.id)
                })
                .map(|binding| (binding.stmt, binding.decl)),
        )
        .collect();
    let moved_defines: HashSet<(usize, usize)> = namespaces
        .iter()
        .chain(&unread)
        .flat_map(|id| define_sites[id].iter().copied())
        .collect();
    let loaded_ids: HashMap<Id, Expr> = bindings
        .iter()
        .filter(|binding| loaded.contains(&binding.id))
        .filter_map(|binding| Some((binding.id.clone(), binding.module_id.clone()?)))
        .collect();
    let loaded_module_ids: Vec<Expr> = loaded_ids.values().cloned().collect();
    drop(stmts);

    // Rewrite: accessor calls and wrappers called where they are built.
    let mut targets: HashMap<Id, Expr> = HashMap::default();
    let mut inline_index: HashMap<Span, usize> = HashMap::default();
    for (index, module) in wrapped.iter().enumerate() {
        match &module.id {
            Some(id) => {
                targets.insert(id.clone(), str_lit(&table_wrapper_id(index)));
            }
            None => {
                inline_index.insert(module.function_span, index);
            }
        }
    }
    for binding in &bindings {
        if binding.kind == ScopeBindingKind::Accessor {
            targets.insert(binding.id.clone(), binding.module_id.clone()?);
        }
    }
    let mut rewriter = TableCallRewriter {
        targets: &targets,
        inline: &inline_index,
        require: &require,
        inline_factories: HashMap::default(),
    };
    bootstrap.visit_mut_with(&mut rewriter);
    let mut factories: Vec<Option<Factory>> = (0..wrapped.len())
        .map(|index| rewriter.inline_factories.remove(&index))
        .collect();

    // Move out the namespace objects' calls and the declarations.
    let scope_body = scope_stmts_mut(&mut bootstrap, &scope)?;
    let mut namespace_calls: HashMap<Id, Vec<Expr>> = HashMap::default();
    for (index, stmt) in scope_body.iter_mut().enumerate() {
        match stmt {
            Stmt::Expr(ExprStmt { expr, span })
                if moved_defines.iter().any(|(at, _)| *at == index) =>
            {
                let invalid = Expr::Invalid(swc_core::ecma::ast::Invalid { span: DUMMY_SP });
                let elements = match *std::mem::replace(expr, Box::new(invalid)) {
                    Expr::Seq(seq) => seq.exprs,
                    Expr::Paren(paren) => match *paren.expr {
                        Expr::Seq(seq) => seq.exprs,
                        expr => vec![Box::new(expr)],
                    },
                    expr => vec![Box::new(expr)],
                };
                let mut kept = Vec::with_capacity(elements.len());
                for (element, value) in elements.into_iter().enumerate() {
                    if moved_defines.contains(&(index, element)) {
                        let id = namespace_define_target(&value, &require, &namespace_ids)?;
                        namespace_calls.entry(id).or_default().push(*value);
                    } else {
                        kept.push(value);
                    }
                }
                let span = *span;
                let mut out = Vec::new();
                flush_elements(&mut kept, span, &mut out);
                *stmt = out
                    .pop()
                    .unwrap_or(Stmt::Empty(swc_core::ecma::ast::EmptyStmt { span }));
            }
            Stmt::Decl(Decl::Var(var)) if removed.iter().any(|(at, _)| *at == index) => {
                for (position, module) in wrapped.iter().enumerate() {
                    if module.id.is_some() && module.stmt == index {
                        let function = declarator_wrapped_function(var, module.decl)?;
                        factories[position] = Some(function_factory(function)?);
                    }
                }
                let mut decl_index = 0;
                var.decls.retain(|_| {
                    let keep = !removed.contains(&(index, decl_index));
                    decl_index += 1;
                    keep
                });
                if var.decls.is_empty() {
                    *stmt = Stmt::Empty(swc_core::ecma::ast::EmptyStmt { span: var.span });
                }
            }
            _ => {}
        }
    }
    // Bind the stubs; every call left in entry code reads the binding.
    if !stub_bindings.is_empty() {
        let mut bound: HashMap<String, Id> = HashMap::default();
        for &index in stub_bindings.iter().flat_map(|stub| &stub.bind) {
            bound.insert(table_wrapper_id(index), wrapped[index].id.clone()?);
        }
        scope_body.visit_mut_with(&mut ReadBoundLoads {
            require: &require,
            bound: &bound,
        });
        let mut expanded = Vec::with_capacity(scope_body.len() + bound.len());
        for (index, stmt) in std::mem::take(scope_body).into_iter().enumerate() {
            let stubs: Vec<&StubBinding> = stub_bindings
                .iter()
                .filter(|stub| stub.stmt == index)
                .collect();
            if stubs.is_empty() {
                expanded.push(stmt);
                continue;
            }
            let Stmt::Expr(ExprStmt { span, expr }) = stmt else {
                return None;
            };
            let elements = match *expr {
                Expr::Seq(seq) => seq.exprs,
                Expr::Paren(paren) => match *paren.expr {
                    Expr::Seq(seq) => seq.exprs,
                    expr => vec![Box::new(expr)],
                },
                expr => vec![Box::new(expr)],
            };
            let mut pending = Vec::new();
            for (element, value) in elements.into_iter().enumerate() {
                let Some(stub) = stubs.iter().find(|stub| stub.element == element) else {
                    pending.push(value);
                    continue;
                };
                let at = value.span();
                if stub.bind.first() != Some(&stub.module) {
                    pending.push(value);
                }
                flush_elements(&mut pending, span, &mut expanded);
                for &module in &stub.bind {
                    let id = wrapped[module].id.clone()?;
                    expanded.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                        span: at,
                        kind: swc_core::ecma::ast::VarDeclKind::Var,
                        decls: vec![VarDeclarator {
                            span: DUMMY_SP,
                            name: Pat::Ident(Ident::new(id.0, DUMMY_SP, id.1).into()),
                            init: Some(Box::new(require_call_expr(
                                &require,
                                str_lit(&table_wrapper_id(module)),
                            ))),
                            definite: false,
                        }],
                        ..Default::default()
                    }))));
                }
            }
            flush_elements(&mut pending, span, &mut expanded);
        }
        *scope_body = expanded;
    }
    scope_body.retain(|stmt| !matches!(stmt, Stmt::Empty(_)));

    // Reads of the moved namespace objects, everywhere, and of loaded table
    // modules, in the moved code.
    let namespace_reads: HashMap<Id, Expr> = namespaces
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let module_id = str_lit(&table_wrapper_id(wrapped.len() + index));
            (id.clone(), require_call_expr(&require, module_id))
        })
        .collect();
    let mut replace_namespaces = ReplaceReads {
        reads: &namespace_reads,
    };
    bootstrap.visit_mut_with(&mut replace_namespaces);
    let mut namespace_factories: Vec<Factory> = Vec::with_capacity(namespaces.len());
    for id in &namespaces {
        let calls = namespace_calls.remove(id)?;
        namespace_factories.push(Factory {
            params: Vec::new(),
            body: calls
                .into_iter()
                .map(|call| {
                    Stmt::Expr(ExprStmt {
                        span: call.span(),
                        expr: Box::new(call),
                    })
                })
                .collect(),
            is_arrow: false,
            span: DUMMY_SP,
        });
    }
    let loaded_reads: HashMap<Id, Expr> = loaded_ids
        .iter()
        .map(|(id, module_id)| (id.clone(), require_call_expr(&require, module_id.clone())))
        .collect();
    let mut factories: Vec<Factory> = factories.into_iter().collect::<Option<Vec<_>>>()?;
    for factory in factories.iter_mut().chain(namespace_factories.iter_mut()) {
        factory.body.visit_mut_with(&mut replace_namespaces);
        factory.body.visit_mut_with(&mut ReplaceReads {
            reads: &loaded_reads,
        });
    }
    let namespace_count = namespace_factories.len();
    factories.extend(namespace_factories);

    let mut rename = RenameBinding {
        id: &require,
        to: &require_sym,
    };
    bootstrap.visit_mut_with(&mut rename);
    bootstrap.visit_mut_with(&mut ClearContexts);

    let mut translated = Vec::with_capacity(factories.len());
    for (index, (factory, filename)) in factories.into_iter().zip(filenames).enumerate() {
        let is_namespace = index >= wrapped.len();
        let Factory {
            mut params,
            mut body,
            ..
        } = factory;
        body.visit_mut_with(&mut rename);
        let mut names = IdentNameCollector::default();
        body.visit_with(&mut names);
        params.visit_with(&mut names);
        names.0.insert(require_sym.clone());
        for base in ["__webpack_module__", "__webpack_exports__"]
            .into_iter()
            .skip(params.len())
        {
            let name = fresh_name(base, &names.0);
            names.0.insert(name.clone());
            params.push(Pat::Ident(Ident::new_no_ctxt(name, DUMMY_SP).into()));
        }
        params.push(Pat::Ident(
            Ident::new_no_ctxt(require_sym.clone(), DUMMY_SP).into(),
        ));
        if is_namespace {
            // `R.r(N)` / `R.d(N, …)` define the module's own exports.
            let Pat::Ident(exports) = &params[1] else {
                return None;
            };
            let exports = Expr::Ident(exports.id.clone());
            for stmt in &mut body {
                let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
                    return None;
                };
                let Expr::Call(call) = strip_parens_mut(expr) else {
                    return None;
                };
                *call.args.first_mut()?.expr = exports.clone();
            }
        }
        // The body ran inside the strict bootstrap.
        if strict && !has_use_strict_directive(&body) {
            body.insert(0, use_strict_directive());
        }
        body.visit_mut_with(&mut ClearContexts);
        params.visit_mut_with(&mut ClearContexts);
        translated.push(TranslatedWebpackFactory {
            id: table_wrapper_id(index),
            filename,
            params,
            body,
        });
    }
    debug_assert_eq!(translated.len(), wrapped.len() + namespace_count);

    Some(TableWrapperSplit {
        bootstrap: FunctionBody {
            span: bootstrap_body.span,
            stmts: bootstrap,
        },
        factories: translated,
        require_sym,
        loaded_module_ids,
    })
}

/// An import-position stub `X();` of a bootstrap with a module table, and
/// the wrapped modules bound right at it.
struct StubBinding {
    stmt: usize,
    element: usize,
    /// The wrapped module the stub loads.
    module: usize,
    /// The wrapped modules whose load the stub completes and whose exports
    /// settle on load, `module` first when it is one of them.
    bind: Vec<usize>,
}

/// The stubs among `entry`, the scope's statements from `from` on, that can
/// bind their module (see [`bind_import_stubs`]): every statement before a
/// stub runs nothing that can call an accessor, so the stub is the first
/// call. Loading a table module counts as running nothing: a table module
/// cannot reach the bootstrap's accessors.
#[allow(clippy::too_many_arguments)]
fn table_import_stubs(
    entry: &[&Stmt],
    from: usize,
    wrapped: &[TableWrapped],
    settles: &[bool],
    loads: &[Vec<usize>],
    table_loads: &HashSet<Id>,
    require: &Id,
    top_ctxt: SyntaxContext,
) -> Vec<StubBinding> {
    let positions: HashMap<Id, usize> = wrapped
        .iter()
        .enumerate()
        .filter_map(|(index, module)| Some((module.id.clone()?, index)))
        .collect();
    let wrapper_ids: HashSet<Id> = positions.keys().cloned().collect();
    let quiet = |expr: &Expr| {
        is_inert_expr(expr, top_ctxt)
            || matches!(strip_parens(expr), Expr::Call(call)
                if accessor_call_target(call, table_loads).is_some())
            || table_load_module_id(expr, require).is_some()
    };
    let mut loaded: HashSet<usize> = HashSet::default();
    let mut out = Vec::new();
    for (offset, stmt) in entry.iter().enumerate() {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            let quiet_stmt = match stmt {
                Stmt::Decl(Decl::Var(var)) => var.decls.iter().all(|decl| {
                    matches!(decl.name, Pat::Ident(_)) && decl.init.as_deref().is_none_or(quiet)
                }),
                stmt => is_inert_stmt(stmt, top_ctxt),
            };
            if !quiet_stmt {
                break;
            }
            continue;
        };
        let elements: Vec<&Expr> = match strip_parens(expr) {
            Expr::Seq(seq) => seq.exprs.iter().map(|expr| &**expr).collect(),
            expr => vec![expr],
        };
        for (element, value) in elements.into_iter().enumerate() {
            let target = match strip_parens(value) {
                Expr::Call(call) => {
                    accessor_call_target(call, &wrapper_ids).map(|ident| positions[&ident.to_id()])
                }
                _ => None,
            };
            let Some(module) = target else {
                if !quiet(value) {
                    return out;
                }
                continue;
            };
            if loaded.contains(&module) {
                continue;
            }
            let mut completed = vec![module];
            let mut next = 0;
            while let Some(current) = completed.get(next).copied() {
                next += 1;
                for &other in &loads[current] {
                    if !completed.contains(&other) && !loaded.contains(&other) {
                        completed.push(other);
                    }
                }
            }
            let bind: Vec<usize> = completed
                .iter()
                .copied()
                .filter(|module| settles[*module])
                .collect();
            loaded.extend(completed);
            if !bind.is_empty() {
                out.push(StubBinding {
                    stmt: from + offset,
                    element,
                    module,
                    bind,
                });
            }
        }
    }
    out
}

/// Whether an identifier of `stmts` is one of `ids`.
fn stmts_mention(stmts: &[&Stmt], ids: &HashSet<Id>) -> bool {
    struct Finder<'a> {
        ids: &'a HashSet<Id>,
        found: bool,
    }
    impl Visit for Finder<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            self.found |= self.ids.contains(&ident.to_id());
        }
    }
    let mut finder = Finder { ids, found: false };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
    }
    finder.found
}

/// Whether `stmt` reads the member `require.<prop>`.
fn stmt_uses_member(stmt: &Stmt, require: &Id, prop: &str) -> bool {
    struct Finder<'a> {
        require: &'a Id,
        prop: &'a str,
        found: bool,
    }
    impl Visit for Finder<'_> {
        fn visit_member_expr(&mut self, member: &MemberExpr) {
            self.found |= matches!(&*member.obj, Expr::Ident(object) if object.to_id() == *self.require)
                && static_prop_name(&member.prop) == Some(self.prop);
            member.visit_children_with(self);
        }
    }
    let mut finder = Finder {
        require,
        prop,
        found: false,
    };
    stmt.visit_with(&mut finder);
    finder.found
}

/// Replaces each `require("<id>")` of a bound wrapped module with its binding.
struct ReadBoundLoads<'a> {
    require: &'a Id,
    bound: &'a HashMap<String, Id>,
}

impl VisitMut for ReadBoundLoads<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        let Expr::Call(call) = expr else {
            return;
        };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        if !matches!(&**callee, Expr::Ident(ident) if ident.to_id() == *self.require) {
            return;
        }
        let [arg] = call.args.as_slice() else {
            return;
        };
        let Expr::Lit(Lit::Str(module_id)) = &*arg.expr else {
            return;
        };
        let Some(id) = module_id
            .value
            .as_str()
            .and_then(|module_id| self.bound.get(module_id))
        else {
            return;
        };
        *expr = Expr::Ident(Ident::new(id.0.clone(), call.span, id.1));
    }
}

/// Whether the table factory `(params) { body }` replaces `module.exports`
/// only while it loads: its module parameter is used only as
/// `module.exports`, outside every function in it (see
/// [`exports_settle_on_load`]).
pub(super) fn table_factory_settles(params: &[Pat], body: &[Stmt]) -> bool {
    match params.first() {
        None => return true,
        Some(Pat::Ident(_)) => {}
        Some(_) => return false,
    }
    // Resolve the factory on its own, so a local spelled like the module
    // parameter is not taken for it.
    let mut program = Module {
        span: DUMMY_SP,
        body: vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(Expr::Fn(swc_core::ecma::ast::FnExpr {
                ident: None,
                function: Box::new(Function {
                    params: params
                        .iter()
                        .cloned()
                        .map(|pat| swc_core::ecma::ast::Param {
                            span: DUMMY_SP,
                            decorators: Vec::new(),
                            pat,
                        })
                        .collect(),
                    body: Some(FunctionBody {
                        span: DUMMY_SP,
                        stmts: body.to_vec(),
                    }),
                    ..Default::default()
                }),
            })),
        }))],
        shebang: None,
    };
    program.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &program.body[0] else {
        return false;
    };
    let Expr::Fn(function) = &**expr else {
        return false;
    };
    let Some(Pat::Ident(module)) = function.function.params.first().map(|param| &param.pat) else {
        return false;
    };
    let Some(body) = &function.function.body else {
        return false;
    };
    let mut scan = ModuleObjectScan {
        module: module.id.to_id(),
        depth: 0,
        settles: true,
    };
    body.stmts.visit_with(&mut scan);
    scan.settles
}

/// Where the wrappers of a bootstrap with a module table are declared.
enum WrapperScope {
    /// The bootstrap's top level.
    Top,
    /// The body of the startup IIFE that is the bootstrap's last statement,
    /// or the last element of its comma sequence.
    Iife {
        stmt: usize,
        element: Option<usize>,
        body_span: Span,
    },
}

/// Whether a top-level statement of `stmts` builds a wrapper with
/// `R.<prop>(function () { … })`.
fn scope_builds_wrappers(stmts: &[Stmt], require: &Id, prop: &Atom) -> bool {
    stmts.iter().any(|stmt| {
        top_level_values(stmt).into_iter().any(|value| {
            is_wrapper_build(Some(value), require, prop)
                || inline_called_wrapper(value, require, prop).is_some()
        })
    })
}

fn is_wrapper_build(expr: Option<&Expr>, require: &Id, prop: &Atom) -> bool {
    let Some(Expr::Call(call)) = expr.map(strip_parens) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    factory_arg(call).is_some()
        && matches!(strip_parens(callee), Expr::Member(member)
            if matches!(&*member.obj, Expr::Ident(object) if object.to_id() == *require)
                && static_prop_name(&member.prop) == Some(prop.as_str()))
}

/// The namespace object `N` of `R.r(N)` or `R.d(N, { … })`.
fn namespace_define_target(expr: &Expr, require: &Id, namespaces: &HashSet<Id>) -> Option<Id> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    namespace_define_call_target(call, require, namespaces)
}

fn namespace_define_call_target(
    call: &CallExpr,
    require: &Id,
    namespaces: &HashSet<Id>,
) -> Option<Id> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    if !matches!(&*member.obj, Expr::Ident(object) if object.to_id() == *require)
        || !matches!(static_prop_name(&member.prop), Some("r" | "d"))
    {
        return None;
    }
    let target = call.args.first()?;
    match &*target.expr {
        Expr::Ident(ident) if target.spread.is_none() && namespaces.contains(&ident.to_id()) => {
            Some(ident.to_id())
        }
        _ => None,
    }
}

/// The `element`-th top-level expression of an expression statement.
fn top_level_element(stmt: &Stmt, element: usize) -> Option<&Expr> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    match strip_parens(expr) {
        Expr::Seq(seq) => seq.exprs.get(element).map(|expr| &**expr),
        expr => (element == 0).then_some(expr),
    }
}

/// The last top-level startup IIFE `(() => { … })()` (or
/// `(function () { … })()`) of the bootstrap that builds wrappers: a
/// statement of its own or one element of a comma sequence. Code may follow
/// it, such as an `output.library` assignment
/// (`Lib = __webpack_exports__`); the IIFE still runs once at its position,
/// and nothing after it sees its bindings. A wrapper built anywhere else
/// fails the `.cw` use count, so the choice cannot miss one.
fn startup_iife_scope(bootstrap: &[Stmt]) -> Option<WrapperScope> {
    bootstrap
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, stmt)| {
            let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
                return None;
            };
            let elements: Vec<(Option<usize>, &Expr)> = match strip_parens(expr) {
                Expr::Seq(seq) => seq
                    .exprs
                    .iter()
                    .enumerate()
                    .map(|(idx, element)| (Some(idx), &**element))
                    .collect(),
                expr => vec![(None, expr)],
            };
            elements.into_iter().rev().find_map(|(element, value)| {
                let body = iife_body(value)?;
                builds_wrappers(body).then_some(WrapperScope::Iife {
                    stmt: index,
                    element,
                    body_span: body.span,
                })
            })
        })
}

/// Whether `body` reads a `.cw` member at all (the precise use checks run
/// later).
fn builds_wrappers(body: &FunctionBody) -> bool {
    struct CwMember(bool);
    impl Visit for CwMember {
        fn visit_member_expr(&mut self, member: &MemberExpr) {
            self.0 |= static_prop_name(&member.prop) == Some("cw");
            member.visit_children_with(self);
        }
    }
    let mut finder = CwMember(false);
    body.visit_with(&mut finder);
    finder.0
}

/// The body of a parameterless, argument-less, sync IIFE.
fn iife_body(expr: &Expr) -> Option<&FunctionBody> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !call.args.is_empty() {
        return None;
    }
    match strip_parens(callee) {
        Expr::Arrow(arrow) if !arrow.is_async && !arrow.is_generator && arrow.params.is_empty() => {
            match &*arrow.body {
                ArrowFunctionBody::FunctionBody(body) => Some(body),
                ArrowFunctionBody::Expr(_) => None,
            }
        }
        Expr::Fn(function)
            if is_plain_function(&function.function) && function.function.params.is_empty() =>
        {
            function.function.body.as_ref()
        }
        _ => None,
    }
}

fn iife_body_mut(expr: &mut Expr) -> Option<&mut FunctionBody> {
    let Expr::Call(call) = strip_parens_mut(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &mut call.callee else {
        return None;
    };
    match strip_parens_mut(callee) {
        Expr::Arrow(arrow) => match &mut *arrow.body {
            ArrowFunctionBody::FunctionBody(body) => Some(body),
            ArrowFunctionBody::Expr(_) => None,
        },
        Expr::Fn(function) => function.function.body.as_mut(),
        _ => None,
    }
}

fn scope_stmts<'a>(bootstrap: &'a [Stmt], scope: &WrapperScope) -> Option<&'a [Stmt]> {
    let WrapperScope::Iife { stmt, element, .. } = scope else {
        return Some(bootstrap);
    };
    let Stmt::Expr(ExprStmt { expr, .. }) = bootstrap.get(*stmt)? else {
        return None;
    };
    let value = match (element, strip_parens(expr)) {
        (Some(element), Expr::Seq(seq)) => &**seq.exprs.get(*element)?,
        (None, expr) => expr,
        _ => return None,
    };
    Some(&iife_body(value)?.stmts)
}

fn scope_stmts_mut<'a>(
    bootstrap: &'a mut Vec<Stmt>,
    scope: &WrapperScope,
) -> Option<&'a mut Vec<Stmt>> {
    let WrapperScope::Iife { stmt, element, .. } = scope else {
        return Some(bootstrap);
    };
    let Stmt::Expr(ExprStmt { expr, .. }) = bootstrap.get_mut(*stmt)? else {
        return None;
    };
    let value = match (element, strip_parens_mut(expr)) {
        (Some(element), Expr::Seq(seq)) => &mut **seq.exprs.get_mut(*element)?,
        (None, expr) => expr,
        _ => return None,
    };
    Some(&mut iife_body_mut(value)?.stmts)
}

/// A statement among the accessor and namespace object declarations: it only
/// defines values ([`is_inert_expr`]) and calls members of `R`, webpack's
/// runtime helpers (`R.cw(function () { … })`, `R.d(ns, { a: () => X().a })`,
/// `R.r(ns)`), which call none of the functions they are given.
fn is_definition_stmt(stmt: &Stmt, require: &Id, top_ctxt: SyntaxContext) -> bool {
    let defines = |expr: &Expr| {
        is_inert_expr(expr, top_ctxt)
            || matches!(strip_parens(expr), Expr::Call(call)
                if matches!(&call.callee, Callee::Expr(callee)
                    if matches!(strip_parens(callee), Expr::Member(member)
                        if matches!(&*member.obj, Expr::Ident(object) if object.to_id() == *require)
                            && static_prop_name(&member.prop).is_some()))
                    && call.args.iter().all(|arg| arg.spread.is_none() && is_inert_expr(&arg.expr, top_ctxt)))
    };
    match stmt {
        Stmt::Empty(_) | Stmt::Decl(Decl::Fn(_)) => true,
        Stmt::Decl(Decl::Var(var)) => var.decls.iter().all(|decl| {
            matches!(decl.name, Pat::Ident(_)) && decl.init.as_deref().is_none_or(defines)
        }),
        Stmt::Expr(ExprStmt { expr, .. }) => match strip_parens(expr) {
            Expr::Seq(seq) => seq.exprs.iter().all(|expr| defines(expr)),
            expr => defines(expr),
        },
        _ => false,
    }
}

/// Whether `stmts` read no binding but `require`, globals, the bindings they
/// declare themselves, and zero-argument calls of the table module
/// accessors `accessors`: then running them runs no wrapped body and no
/// namespace getter, which only code declared elsewhere in the bootstrap
/// reaches (a table module cannot).
fn stmts_read_only_their_own(
    stmts: &[&Stmt],
    accessors: &HashSet<Id>,
    require: &Id,
    unresolved_mark: Mark,
) -> bool {
    let mut declared = DeclaredIds::default();
    for stmt in stmts {
        stmt.visit_with(&mut declared);
    }
    let mut scan = OuterReadScan {
        declared: &declared.0,
        callable: accessors,
        require,
        unresolved_mark,
        reads: Vec::new(),
    };
    for stmt in stmts {
        stmt.visit_with(&mut scan);
    }
    scan.reads.is_empty()
}

/// What a wrapped function of a bootstrap with a module table reads around
/// it, other than `R` and zero-argument calls of `callable`. `None` when it
/// cannot run as the factory `(module, exports, R)`: more than two
/// parameters or one that is not plain, or `arguments`, `new.target`, or
/// `return` at its level (and `this` in an arrow, the bootstrap's).
fn table_factory_reads(
    function: &Expr,
    callable: &HashSet<Id>,
    require: &Id,
    unresolved_mark: Mark,
) -> Option<Vec<Id>> {
    let factory = function_factory(function)?;
    if factory.params.len() > 2
        || !factory
            .params
            .iter()
            .all(|param| matches!(param, Pat::Ident(_)))
    {
        return None;
    }
    let observes_boundary = if factory.is_arrow {
        stmts_have_function_level_special_bindings(&factory.body)
    } else {
        stmts_have_function_level_arguments_or_new_target(&factory.body)
    };
    if observes_boundary || stmts_have_function_level_return(&factory.body) {
        return None;
    }
    expr_outer_reads(function, callable, require, unresolved_mark)
}

/// The bindings `expr` reads that it does not declare, other than `require`
/// and zero-argument calls of `callable`.
fn expr_outer_reads(
    expr: &Expr,
    callable: &HashSet<Id>,
    require: &Id,
    unresolved_mark: Mark,
) -> Option<Vec<Id>> {
    let mut declared = DeclaredIds::default();
    expr.visit_with(&mut declared);
    let mut scan = OuterReadScan {
        declared: &declared.0,
        callable,
        require,
        unresolved_mark,
        reads: Vec::new(),
    };
    expr.visit_with(&mut scan);
    Some(scan.reads)
}

/// Every binding a node declares, at any depth.
#[derive(Default)]
struct DeclaredIds(HashSet<Id>);

impl DeclaredIds {
    fn pat(&mut self, pat: &Pat) {
        struct Bindings<'a>(&'a mut HashSet<Id>);
        impl Visit for Bindings<'_> {
            fn visit_binding_ident(&mut self, binding: &swc_core::ecma::ast::BindingIdent) {
                self.0.insert(binding.id.to_id());
            }
            // A default value is an expression, not a declaration.
            fn visit_expr(&mut self, _: &Expr) {}
        }
        pat.visit_with(&mut Bindings(&mut self.0));
    }
}

impl Visit for DeclaredIds {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        self.pat(&decl.name);
        decl.visit_children_with(self);
    }

    fn visit_param(&mut self, param: &swc_core::ecma::ast::Param) {
        self.pat(&param.pat);
        param.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        for param in &arrow.params {
            self.pat(param);
        }
        arrow.visit_children_with(self);
    }

    fn visit_catch_clause(&mut self, clause: &swc_core::ecma::ast::CatchClause) {
        if let Some(param) = &clause.param {
            self.pat(param);
        }
        clause.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
        self.0.insert(decl.ident.to_id());
        decl.visit_children_with(self);
    }

    fn visit_fn_expr(&mut self, function: &swc_core::ecma::ast::FnExpr) {
        if let Some(ident) = &function.ident {
            self.0.insert(ident.to_id());
        }
        function.visit_children_with(self);
    }

    fn visit_class_decl(&mut self, decl: &swc_core::ecma::ast::ClassDecl) {
        self.0.insert(decl.ident.to_id());
        decl.visit_children_with(self);
    }

    fn visit_class_expr(&mut self, class: &swc_core::ecma::ast::ClassExpr) {
        if let Some(ident) = &class.ident {
            self.0.insert(ident.to_id());
        }
        class.visit_children_with(self);
    }
}

/// The bindings a node reads that it does not declare (`declared`), other
/// than `require` and zero-argument calls of `callable`.
struct OuterReadScan<'a> {
    declared: &'a HashSet<Id>,
    callable: &'a HashSet<Id>,
    require: &'a Id,
    unresolved_mark: Mark,
    reads: Vec<Id>,
}

impl Visit for OuterReadScan<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if accessor_call_target(call, self.callable).is_some() {
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        let id = ident.to_id();
        if ident.ctxt.outer() != self.unresolved_mark
            && id != *self.require
            && !self.declared.contains(&id)
            && !self.reads.contains(&id)
        {
            self.reads.push(id);
        }
    }

    // Labels are not bindings.
    fn visit_labeled_stmt(&mut self, stmt: &swc_core::ecma::ast::LabeledStmt) {
        stmt.body.visit_with(self);
    }

    fn visit_break_stmt(&mut self, _: &swc_core::ecma::ast::BreakStmt) {}

    fn visit_continue_stmt(&mut self, _: &swc_core::ecma::ast::ContinueStmt) {}
}

/// The values a top-level statement evaluates once, in place: the expression
/// of an expression statement (each element of a comma sequence), and the
/// initializers of a declaration.
fn top_level_values(stmt: &Stmt) -> Vec<&Expr> {
    match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => match strip_parens(expr) {
            Expr::Seq(seq) => seq.exprs.iter().map(|expr| &**expr).collect(),
            expr => vec![expr],
        },
        Stmt::Decl(Decl::Var(var)) => var
            .decls
            .iter()
            .filter_map(|decl| decl.init.as_deref())
            .collect(),
        _ => Vec::new(),
    }
}

/// The wrapped function of `R.<prop>(function () { … })()`.
fn inline_called_wrapper<'a>(expr: &'a Expr, require: &Id, prop: &Atom) -> Option<&'a Expr> {
    let Expr::Call(outer) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &outer.callee else {
        return None;
    };
    let Expr::Call(inner) = strip_parens(callee) else {
        return None;
    };
    let Callee::Expr(runtime) = &inner.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(runtime) else {
        return None;
    };
    (outer.args.is_empty()
        && matches!(&*member.obj, Expr::Ident(object) if object.to_id() == *require)
        && static_prop_name(&member.prop) == Some(prop.as_str()))
    .then(|| factory_arg(inner))
    .flatten()
}

/// The wrapped function of the declarator `X = R.<prop>(function () { … })`.
fn declarator_wrapped_function(var: &VarDecl, decl: usize) -> Option<&Expr> {
    let Expr::Call(call) = strip_parens(var.decls.get(decl)?.init.as_deref()?) else {
        return None;
    };
    factory_arg(call)
}

fn wrapped_function<'a>(
    stmt: &'a Stmt,
    module: &TableWrapped,
    require: &Id,
    prop: &Atom,
) -> Option<&'a Expr> {
    match (&module.id, stmt) {
        (Some(_), Stmt::Decl(Decl::Var(var))) => declarator_wrapped_function(var, module.decl),
        (Some(_), _) => None,
        (None, stmt) => top_level_values(stmt)
            .into_iter()
            .filter_map(|value| inline_called_wrapper(value, require, prop))
            .find(|function| function_span(function) == Some(module.function_span)),
    }
}

fn function_span(function: &Expr) -> Option<Span> {
    match function {
        Expr::Arrow(arrow) => Some(arrow.span),
        Expr::Fn(function) => Some(function.function.span),
        _ => None,
    }
}

/// The id literal of a table module accessor, `() => R(<id>)`,
/// `() => { return R(<id>); }`, or `function () { return R(<id>); }`.
fn table_accessor_module_id(init: &Expr, require: &Id) -> Option<Expr> {
    let returned = match strip_parens(init) {
        Expr::Arrow(arrow) if !arrow.is_async && !arrow.is_generator && arrow.params.is_empty() => {
            match &*arrow.body {
                ArrowFunctionBody::Expr(expr) => &**expr,
                ArrowFunctionBody::FunctionBody(body) => sole_return(&body.stmts)?,
            }
        }
        Expr::Fn(function)
            if function.ident.is_none()
                && is_plain_function(&function.function)
                && function.function.params.is_empty() =>
        {
            sole_return(&function.function.body.as_ref()?.stmts)?
        }
        _ => return None,
    };
    table_load_module_id(returned, require)
}

/// The id literal of `R(<id>)`.
fn table_load_module_id(expr: &Expr, require: &Id) -> Option<Expr> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !matches!(strip_parens(callee), Expr::Ident(ident) if ident.to_id() == *require) {
        return None;
    }
    let [arg] = call.args.as_slice() else {
        return None;
    };
    match strip_parens(&arg.expr) {
        id @ Expr::Lit(Lit::Num(_) | Lit::Str(_)) if arg.spread.is_none() => Some(id.clone()),
        _ => None,
    }
}

fn sole_return(stmts: &[Stmt]) -> Option<&Expr> {
    match stmts {
        [Stmt::Return(ret)] => ret.arg.as_deref(),
        _ => None,
    }
}

/// The wrapped module's file: the path of a `// MODULE: <path>` comment
/// before its own statement, otherwise `module-cw-<index>.js`. The table
/// path makes the names unique.
fn table_wrapper_filename(
    cm: &SourceMap,
    stmt: &Stmt,
    module: &TableWrapped,
    index: usize,
) -> String {
    let hint = match (stmt, &module.id) {
        (Stmt::Decl(Decl::Var(var)), Some(_)) if var.decls.len() == 1 => {
            module_comment_path(cm, var.span)
        }
        (Stmt::Expr(ExprStmt { span, expr }), None)
            if !matches!(strip_parens(expr), Expr::Seq(_)) =>
        {
            module_comment_path(cm, *span)
        }
        _ => None,
    };
    webpack_module_filename(&hint.unwrap_or_else(|| format!("cw-{index}")))
}

fn str_lit(value: &str) -> Expr {
    Expr::Lit(Lit::Str(Str {
        span: DUMMY_SP,
        value: value.into(),
        raw: None,
    }))
}

/// `require(<module_id>)`, with `require` the binding `require`.
fn require_call_expr(require: &Id, module_id: Expr) -> Expr {
    Expr::Call(CallExpr {
        span: DUMMY_SP,
        callee: Callee::Expr(Box::new(Expr::Ident(Ident::new(
            require.0.clone(),
            DUMMY_SP,
            require.1,
        )))),
        args: vec![module_id.into()],
        ..Default::default()
    })
}

fn fresh_name(base: &str, taken: &HashSet<Atom>) -> Atom {
    let mut name = Atom::from(base);
    let mut suffix = 1;
    while taken.contains(&name) {
        name = Atom::from(format!("{base}_{suffix}"));
        suffix += 1;
    }
    name
}

/// How the bootstrap uses the wrapper scope's bindings (`tracked`) and the
/// require function `require`, by statement of that scope.
struct TableUseScan<'a> {
    tracked: &'a HashSet<Id>,
    /// The tracked namespace object candidates: their `R.r(N)` / `R.d(N, …)`
    /// calls do not count as uses.
    namespaces: &'a HashSet<Id>,
    require: &'a Id,
    prop: &'a Atom,
    /// The wrapped functions that run only when their accessor is called.
    lazy: &'a HashSet<Span>,
    /// The wrapped functions called where they are built: no nesting.
    inline: &'a HashSet<Span>,
    /// The startup IIFE's body when it is the scope; `stmt` counts its
    /// statements, and the IIFE is no nesting.
    scope_body: Option<Span>,
    stmt: usize,
    lazy_depth: usize,
    function_depth: usize,
    /// References other than the declaration and namespace definitions.
    uses: HashMap<Id, usize>,
    declared: HashMap<Id, usize>,
    calls: HashMap<Id, usize>,
    /// Bindings a pattern or an update writes.
    written: HashSet<Id>,
    /// `R.r(N)` / `R.d(N, …)` calls by namespace object.
    defines: HashMap<Id, usize>,
    /// Namespace objects with such a call inside a function.
    nested_defines: HashSet<Id>,
    /// The first statement with a reference outside a lazily run body.
    first_use: HashMap<Id, usize>,
    /// The first statement with a reference outside every function.
    first_eager_use: HashMap<Id, usize>,
    /// `require.<prop>` reads and writes.
    host_uses: usize,
    require_functions: usize,
    /// Whether a pattern or an update writes `require`.
    require_bound: bool,
}

impl TableUseScan<'_> {
    fn nested(&mut self, span: Span, body: Option<Span>, visit: impl FnOnce(&mut Self)) {
        let lazy = usize::from(self.lazy.contains(&span));
        let function = usize::from(
            !self.inline.contains(&span) && (body.is_none() || body != self.scope_body),
        );
        self.lazy_depth += lazy;
        self.function_depth += function;
        visit(self);
        self.lazy_depth -= lazy;
        self.function_depth -= function;
    }
}

impl Visit for TableUseScan<'_> {
    fn visit_function_body(&mut self, body: &FunctionBody) {
        if Some(body.span) != self.scope_body {
            body.visit_children_with(self);
            return;
        }
        for (index, stmt) in body.stmts.iter().enumerate() {
            self.stmt = index;
            stmt.visit_with(self);
        }
    }

    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        if let Pat::Ident(name) = &decl.name {
            let id = name.id.to_id();
            if self.tracked.contains(&id) {
                *self.declared.entry(id).or_default() += 1;
                decl.init.visit_with(self);
                return;
            }
        }
        decl.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(ident) = accessor_call_target(call, self.tracked) {
            *self.calls.entry(ident.to_id()).or_default() += 1;
        }
        if let Some(id) = namespace_define_call_target(call, self.require, self.namespaces) {
            *self.defines.entry(id.clone()).or_default() += 1;
            if self.function_depth > 0 {
                self.nested_defines.insert(id);
            }
            call.callee.visit_with(self);
            for arg in &call.args[1..] {
                arg.visit_with(self);
            }
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if matches!(&*member.obj, Expr::Ident(object) if object.to_id() == *self.require)
            && static_prop_name(&member.prop) == Some(self.prop.as_str())
        {
            self.host_uses += 1;
        }
        member.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
        if decl.ident.to_id() == *self.require {
            self.require_functions += 1;
        }
        decl.visit_children_with(self);
    }

    fn visit_binding_ident(&mut self, binding: &swc_core::ecma::ast::BindingIdent) {
        let id = binding.id.to_id();
        self.require_bound |= id == *self.require;
        if self.tracked.contains(&id) {
            self.written.insert(id);
        }
        binding.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        if let Expr::Ident(ident) = &*update.arg {
            let id = ident.to_id();
            self.require_bound |= id == *self.require;
            if self.tracked.contains(&id) {
                self.written.insert(id);
            }
        }
        update.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        let body = match &*arrow.body {
            ArrowFunctionBody::FunctionBody(body) => Some(body.span),
            ArrowFunctionBody::Expr(_) => None,
        };
        self.nested(arrow.span, body, |scan| arrow.visit_children_with(scan));
    }

    fn visit_function(&mut self, function: &Function) {
        let body = function.body.as_ref().map(|body| body.span);
        self.nested(function.span, body, |scan| {
            function.visit_children_with(scan)
        });
    }

    fn visit_ident(&mut self, ident: &Ident) {
        let id = ident.to_id();
        if self.tracked.contains(&id) {
            *self.uses.entry(id.clone()).or_default() += 1;
            if self.lazy_depth == 0 {
                self.first_use.entry(id.clone()).or_insert(self.stmt);
            }
            if self.function_depth == 0 {
                self.first_eager_use.entry(id).or_insert(self.stmt);
            }
        }
    }
}

/// Every name spelled by an identifier that is not the binding `id`.
struct ForeignSpellings<'a> {
    id: &'a Id,
    syms: HashSet<Atom>,
}

impl Visit for ForeignSpellings<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.to_id() != *self.id {
            self.syms.insert(ident.sym.clone());
        }
    }
}

/// Rewrites each zero-argument accessor call to `require(<id>)` and each
/// wrapper called where it is built to `require("<its id>")`, taking that
/// wrapper's function.
struct TableCallRewriter<'a> {
    targets: &'a HashMap<Id, Expr>,
    inline: &'a HashMap<Span, usize>,
    require: &'a Id,
    inline_factories: HashMap<usize, Factory>,
}

impl VisitMut for TableCallRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        let Expr::Call(call) = expr else {
            return;
        };
        if !call.args.is_empty() {
            return;
        }
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let module_id = match strip_parens(callee) {
            Expr::Ident(ident) => match self.targets.get(&ident.to_id()) {
                Some(module_id) => module_id.clone(),
                None => return,
            },
            Expr::Call(inner) => {
                let Some(function) = factory_arg(inner) else {
                    return;
                };
                let Some(&index) = function_span(function).and_then(|span| self.inline.get(&span))
                else {
                    return;
                };
                let Some(factory) = function_factory(function) else {
                    return;
                };
                self.inline_factories.insert(index, factory);
                str_lit(&table_wrapper_id(index))
            }
            _ => return,
        };
        let span = call.span;
        let mut replacement = require_call_expr(self.require, module_id);
        if let Expr::Call(call) = &mut replacement {
            call.span = span;
        }
        *expr = replacement;
    }
}

/// Replaces each read of a binding in `reads` with its expression.
struct ReplaceReads<'a> {
    reads: &'a HashMap<Id, Expr>,
}

impl VisitMut for ReplaceReads<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Ident(ident) = expr {
            if let Some(replacement) = self.reads.get(&ident.to_id()) {
                *expr = replacement.clone();
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }

    fn visit_mut_prop(&mut self, prop: &mut Prop) {
        if let Prop::Shorthand(ident) = prop {
            if let Some(replacement) = self.reads.get(&ident.to_id()) {
                *prop = Prop::KeyValue(swc_core::ecma::ast::KeyValueProp {
                    key: PropName::Ident(ident.clone().into()),
                    value: Box::new(replacement.clone()),
                });
                return;
            }
        }
        prop.visit_mut_children_with(self);
    }
}

/// Renames every reference to the binding `id` to `to`.
struct RenameBinding<'a> {
    id: &'a Id,
    to: &'a Atom,
}

impl VisitMut for RenameBinding<'_> {
    fn visit_mut_ident(&mut self, ident: &mut Ident) {
        if ident.to_id() == *self.id {
            ident.sym = self.to.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::GLOBALS;

    fn first_expr(source: &str) -> bool {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let module = super::super::parse_es_module(source, "test.js", cm).unwrap();
            let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &module.body[0] else {
                panic!("expected an expression statement");
            };
            is_cw_runtime(expr)
        })
    }

    #[test]
    fn recognizes_the_unminified_and_minified_runtime() {
        assert!(first_expr(
            "(body) => { var mod; return () => { if (body) { var fn = body; body = 0; \
             mod = { exports: {} }; fn.call(mod.exports, mod, mod.exports); } \
             return mod.exports; }; };"
        ));
        assert!(first_expr(
            "(r=>{var t;return()=>{if(r){var e=r;r=0,t={exports:{}},e.call(t.exports,t,t.exports)}return t.exports}});"
        ));
        assert!(first_expr(
            "(function(r){var t;return function(){if(r){var e=r;r=0,t={exports:{}},e.call(t.exports,t,t.exports)}return t.exports}});"
        ));
    }

    #[test]
    fn rejects_a_runtime_that_differs_in_one_step() {
        // Not cleared: the body would run on every call.
        assert!(!first_expr(
            "(r=>{var t;return()=>{if(r){var e=r;t={exports:{}},e.call(t.exports,t,t.exports)}return t.exports}});"
        ));
        // Arguments swapped.
        assert!(!first_expr(
            "(r=>{var t;return()=>{if(r){var e=r;r=0,t={exports:{}},e.call(t.exports,t.exports,t)}return t.exports}});"
        ));
        // Returns the module object instead of its exports.
        assert!(!first_expr(
            "(r=>{var t;return()=>{if(r){var e=r;r=0,t={exports:{}},e.call(t.exports,t,t.exports)}return t}});"
        ));
    }
}
