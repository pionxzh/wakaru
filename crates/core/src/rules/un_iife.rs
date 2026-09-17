use crate::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, BindingIdent, CallExpr, Callee, CatchClause, ClassDecl,
    Constructor, Decl, Expr, ExprOrSpread, FnDecl, Function, FunctionBody, GetterProp, Ident, Lit,
    MemberProp, MethodProp, Module, ObjectPatProp, Param, ParamOrTsParamProp, Pat, SetterProp,
    Stmt, ThisExpr, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;

use super::eval_utils::{js_source_mentions_binding, module_has_with_stmt, DirectEvalAnalyzer};
use super::rename_utils::{rename_bindings, BindingRename, BindingRenamer};
use super::RewriteLevel;

pub struct UnIife {
    level: RewriteLevel,
    /// A `with` statement anywhere in the module: param renames and literal
    /// extraction rebind names the `with` object could supply at runtime, so
    /// they are skipped module-wide (docs/rewrite-assumptions.md). Direct eval
    /// is handled per IIFE body by `plan_param_rewrites`.
    with_statement_present: bool,
}

impl UnIife {
    pub fn new(level: RewriteLevel) -> Self {
        Self {
            level,
            with_statement_present: false,
        }
    }
}

impl Default for UnIife {
    fn default() -> Self {
        Self::new(RewriteLevel::Standard)
    }
}

impl VisitMut for UnIife {
    fn visit_mut_module(&mut self, module: &mut Module) {
        self.with_statement_present = module_has_with_stmt(module);
        module.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Call(call_expr) = expr {
            // Try to simplify `(() => expr)()` (zero-param, expression-body arrow IIFE with no args)
            if let Some(inner) = try_simplify_arrow_expr_iife(call_expr) {
                *expr = *inner;
                return;
            }
            process_iife(call_expr, self.level, self.with_statement_present);
        }
    }
}

/// Simplifies `(() => expr)()` to `expr` (zero-param arrow with expression body, called with no args).
/// This handles the output of `require.n(r)` to `() => r` after inlining.
fn try_simplify_arrow_expr_iife(call: &CallExpr) -> Option<Box<Expr>> {
    // Must have no arguments
    if !call.args.is_empty() {
        return None;
    }
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let arrow = match callee_expr.as_ref() {
        Expr::Arrow(a) => a,
        Expr::Paren(p) => match p.expr.as_ref() {
            Expr::Arrow(a) => a,
            _ => return None,
        },
        _ => return None,
    };
    // Calling an async arrow wraps its result in a Promise and establishes the
    // async context required by `await`. Replacing the call with its body loses
    // both semantics and can emit invalid top-level `await` in a script.
    if arrow.is_async {
        return None;
    }
    // Must have no params
    if !arrow.params.is_empty() {
        return None;
    }
    // Body must be an expression (not a block)
    match arrow.body.as_ref() {
        ArrowFunctionBody::Expr(e) => Some(e.clone()),
        _ => None,
    }
}

fn process_iife(call: &mut CallExpr, level: RewriteLevel, with_statement_present: bool) {
    // `arrow.call(thisArg, args...)` → `arrow(args...)`. Arrow functions ignore
    // a `.call` `thisArg` (their `this` is always lexical), so the thisArg is
    // dead weight and the resulting arrow IIFE can go through the normal path.
    try_unwrap_dot_call_on_arrow(call);

    if level < RewriteLevel::Standard || with_statement_present {
        return;
    }

    if let Callee::Expr(callee_expr) = &mut call.callee {
        match callee_expr.as_mut() {
            Expr::Fn(fn_expr) => {
                process_fn_iife(
                    &mut fn_expr.function,
                    &mut call.args,
                    fn_expr.ident.as_ref(),
                );
            }
            Expr::Arrow(arrow_expr) => {
                process_arrow_iife(arrow_expr, &mut call.args);
            }
            Expr::Paren(paren) => match paren.expr.as_mut() {
                Expr::Fn(fn_expr) => {
                    process_fn_iife(
                        &mut fn_expr.function,
                        &mut call.args,
                        fn_expr.ident.as_ref(),
                    );
                }
                Expr::Arrow(arrow_expr) => {
                    process_arrow_iife(arrow_expr, &mut call.args);
                }
                _ => {}
            },
            _ => {}
        }
    }
}

/// Rewrite `arrow.call(thisArg, args...)` to `arrow(args...)` in place.
///
/// Only fires when the callee base is an arrow (possibly Paren-wrapped) —
/// for a plain `function`, stripping the thisArg is only safe if the body
/// doesn't reference `this`/`arguments`, and that check is already done by
/// `ArrowFunction`. Once that converts the function to an arrow, `UnIife2`
/// (the second pass) catches this shape.
fn try_unwrap_dot_call_on_arrow(call: &mut CallExpr) -> bool {
    let Callee::Expr(callee_expr) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = callee_expr.as_ref() else {
        return false;
    };
    let MemberProp::Ident(prop) = &member.prop else {
        return false;
    };
    if prop.sym != "call" {
        return false;
    }
    let base_is_arrow = match member.obj.as_ref() {
        Expr::Arrow(_) => true,
        Expr::Paren(p) => matches!(p.expr.as_ref(), Expr::Arrow(_)),
        _ => false,
    };
    if !base_is_arrow {
        return false;
    }
    // Need at least a thisArg, and it must not be a spread — otherwise the
    // subsequent positional args don't line up with the param list.
    if call.args.is_empty() || call.args[0].spread.is_some() {
        return false;
    }

    // Take the arrow base out of the Member expr without cloning the body.
    let placeholder = Box::new(Expr::This(ThisExpr { span: DUMMY_SP }));
    let Callee::Expr(callee_box) = &mut call.callee else {
        unreachable!();
    };
    let Expr::Member(member_mut) = callee_box.as_mut() else {
        unreachable!();
    };
    let arrow_base = std::mem::replace(&mut member_mut.obj, placeholder);
    call.callee = Callee::Expr(arrow_base);
    call.args.remove(0);
    true
}

fn process_fn_iife(
    function: &mut Function,
    args: &mut Vec<ExprOrSpread>,
    fn_expr_name: Option<&Ident>,
) {
    let Some(body) = &mut function.body else {
        return;
    };
    if should_preserve_iife_shape(body, function.params.len(), args.len()) {
        return;
    }
    let preserve_arg_list = body_uses_own_arguments(body);
    process_params_and_args(
        &mut function.params,
        args,
        body,
        preserve_arg_list,
        fn_expr_name,
    );
}

/// Named function expressions bind their name inside the function. Any use of
/// that binding in the body or in parameter initializers — call / `new` /
/// `.call` / escape / `typeof` — or a known eval source that mentions the
/// printed name, means a later invocation can pass a different argument.
/// Literal extraction would freeze the IIFE snapshot.
fn named_fn_expr_is_reused(
    fn_expr_name: Option<&Ident>,
    params: &[Param],
    body: &FunctionBody,
    param_value_refs: &ParamValueRefs,
) -> bool {
    let Some(name) = fn_expr_name else {
        return false;
    };
    let binding = (name.sym.clone(), name.ctxt);
    // Defaults and computed pattern keys see the name with the same binding
    // identity as the body. `b = o` can re-enter after the IIFE snapshot.
    if param_value_refs.refs.contains(&binding) {
        return true;
    }
    let binding_uses = BindingUseIndex::collect_stmts(&body.stmts);
    if binding_uses.use_count(&binding) > 0 {
        return true;
    }
    let mut eval_analyzer = DirectEvalAnalyzer::default();
    params.visit_with(&mut eval_analyzer);
    for stmt in &body.stmts {
        stmt.visit_with(&mut eval_analyzer);
    }
    !eval_analyzer.unknown_direct_eval
        && eval_analyzer
            .known_direct_eval_sources
            .iter()
            .any(|source| js_source_mentions_binding(source, &name.sym))
}

fn process_arrow_iife(arrow: &mut ArrowExpr, args: &mut Vec<ExprOrSpread>) {
    let ArrowFunctionBody::FunctionBody(body) = arrow.body.as_mut() else {
        return;
    };
    if should_preserve_iife_shape(body, arrow.params.len(), args.len()) {
        return;
    }
    // Arrow functions do not have their own `arguments` binding, so removing
    // arrow params cannot change what an `arguments` reference observes.
    process_arrow_params_and_args(&mut arrow.params, args, body, false);
}

fn should_preserve_iife_shape(body: &FunctionBody, param_count: usize, arg_count: usize) -> bool {
    // UnEs6Class detects Babel's inline `_inherits` helper from its original
    // two-param/two-arg IIFE shape. If UnIife rewrites the superclass param
    // first, that later class rewrite can lose its inheritance evidence.
    param_count == 2 && arg_count == 2 && body_contains_object_create(body)
}

fn process_params_and_args(
    params: &mut Vec<Param>,
    args: &mut Vec<ExprOrSpread>,
    body: &mut FunctionBody,
    preserve_arg_list: bool,
    fn_expr_name: Option<&Ident>,
) {
    let param_value_refs = collect_param_value_refs(params);
    // Re-entry uses the same fail-closed path as an observable `arguments`
    // object: keep the parameter so later calls can still pass a value.
    let preserve_arg_list =
        preserve_arg_list || named_fn_expr_is_reused(fn_expr_name, params, body, &param_value_refs);
    let plan = plan_param_rewrites(
        params.len(),
        args,
        body,
        &param_value_refs,
        preserve_arg_list,
        true,
        |i| pat_ident(&params[i].pat).map(|id| (id.sym.clone(), id.ctxt)),
    );

    apply_rename_rewrites(params, body, &plan);

    // Process literal inserts: collect indices to remove, then drop params/args
    // in reverse-index order.
    let mut to_remove: Vec<usize> = plan.literal_inserts.iter().map(|(i, ..)| *i).collect();
    to_remove.sort();
    to_remove.dedup();
    to_remove.reverse();
    for i in to_remove {
        params.remove(i);
        args.remove(i);
    }

    prepend_literal_decls(body, &plan.literal_inserts);
}

fn process_arrow_params_and_args(
    params: &mut Vec<Pat>,
    args: &mut Vec<ExprOrSpread>,
    body: &mut FunctionBody,
    preserve_arg_list: bool,
) {
    let param_value_refs = collect_param_value_refs(params);
    let plan = plan_param_rewrites(
        params.len(),
        args,
        body,
        &param_value_refs,
        preserve_arg_list,
        false,
        |i| pat_ident(&params[i]).map(|id| (id.sym.clone(), id.ctxt)),
    );

    apply_rename_rewrites(params, body, &plan);

    let mut to_remove: Vec<usize> = plan.literal_inserts.iter().map(|(i, ..)| *i).collect();
    to_remove.sort();
    to_remove.dedup();
    to_remove.reverse();
    for i in to_remove {
        params.remove(i);
        args.remove(i);
    }

    prepend_literal_decls(body, &plan.literal_inserts);
}

fn pat_ident(pat: &Pat) -> Option<&Ident> {
    if let Pat::Ident(BindingIdent { id, .. }) = pat {
        Some(id)
    } else {
        None
    }
}

/// Per-param rewrite decisions for an IIFE call. Shared between the regular
/// `Function` and arrow paths since the logic is identical once we abstract
/// param introspection.
#[derive(Default)]
struct RewritePlan {
    /// (idx, old_sym, new_sym, param_ctxt): keep the param, change its sym.
    renames: Vec<(usize, Atom, Atom, SyntaxContext)>,
    /// (idx, sym, ctxt, lit, kind): drop the param + arg and prepend
    /// `const sym = lit` or `let sym = lit` when the param is mutated.
    literal_inserts: Vec<(usize, Atom, SyntaxContext, Lit, VarDeclKind)>,
}

fn plan_param_rewrites<F>(
    param_count: usize,
    args: &[ExprOrSpread],
    body: &FunctionBody,
    param_value_refs: &ParamValueRefs,
    preserve_arg_list: bool,
    params_map_arguments: bool,
    param_at: F,
) -> RewritePlan
where
    F: Fn(usize) -> Option<(Atom, SyntaxContext)>,
{
    let mut plan = RewritePlan::default();
    // Direct eval can read or write any binding in scope by name, including
    // from nested functions that close over the params. Renaming a param or
    // replacing it with a lexical declaration changes what the evaluated
    // source observes, so params mentioned by a known eval source keep their
    // shape — and an unknown source blocks every param.
    let mut eval_analyzer = DirectEvalAnalyzer::default();
    for stmt in &body.stmts {
        stmt.visit_with(&mut eval_analyzer);
    }
    if eval_analyzer.unknown_direct_eval {
        return plan;
    }
    let direct_eval_sources = eval_analyzer.known_direct_eval_sources;
    let arguments_name: Atom = "arguments".into();
    let eval_observes_mapped_arguments = params_map_arguments
        && direct_eval_sources
            .iter()
            .any(|source| js_source_mentions_binding(source, &arguments_name));
    // Duplicate parameter names (legal in sloppy-mode functions) share one
    // binding that holds the value of the last duplicate. Per-index renames
    // rebind body uses to the first occurrence, and literal extraction emits
    // colliding lexical declarations.
    let mut seen_params = HashSet::default();
    for i in 0..param_count {
        if let Some(binding) = param_at(i) {
            if !seen_params.insert(binding) {
                return plan;
            }
        }
    }
    // Printed JavaScript has no `SyntaxContext`, so every new name we introduce
    // must avoid bindings anywhere in the IIFE body. This is conservative for
    // suffix renames, but avoids producing a param name that is shadowed at a
    // nested use site after codegen.
    let mut taken_for_suffix: HashSet<Atom> = collect_all_binding_names(body);
    // Renames also reach default expressions. Reserve all their names,
    // including nested bindings and free references, before choosing a suffix.
    taken_for_suffix.extend(param_value_refs.names.iter().cloned());
    for i in 0..param_count {
        if let Some((sym, _)) = param_at(i) {
            taken_for_suffix.insert(sym);
        }
    }
    let binding_uses = BindingUseIndex::collect_stmts(&body.stmts);
    for (i, arg) in args.iter().enumerate().take(param_count) {
        // A spread argument has a runtime length, so from this position on the
        // syntactic argument index no longer identifies the parameter it
        // initializes. Stop positional reasoning here.
        if arg.spread.is_some() {
            break;
        }
        let Some((param_sym, param_ctxt)) = param_at(i) else {
            continue;
        };
        if param_sym.len() != 1 {
            continue;
        }
        let param_binding = (param_sym.clone(), param_ctxt);
        if direct_eval_sources
            .iter()
            .any(|source| js_source_mentions_binding(source, &param_sym))
        {
            continue;
        }

        match arg.expr.as_ref() {
            Expr::Ident(ident) => {
                if ident.sym.len() <= 1 || ident.sym == param_sym {
                    continue;
                }
                if ident.sym.as_ref() == "undefined" {
                    continue;
                }
                // Keep identifier args as parameters. Dropping the param would
                // turn a call-time snapshot into a live read of the outer
                // binding, which is unsafe for closures and body-side effects.
                let mut taken = taken_for_suffix.clone();
                taken.insert(ident.sym.clone());
                let new_sym = pick_non_conflicting_name(&ident.sym, &taken);
                if direct_eval_sources
                    .iter()
                    .any(|source| js_source_mentions_binding(source, &new_sym))
                {
                    continue;
                }
                taken_for_suffix.remove(&param_sym);
                taken_for_suffix.insert(new_sym.clone());
                plan.renames.push((i, param_sym, new_sym, param_ctxt));
            }
            // A body declaration with the parameter's resolved binding ID is a
            // same-binding `var`/function redeclaration. Removing the parameter
            // and inserting a lexical declaration would either create an early
            // error or change the redeclaration semantics. A sibling parameter
            // default or pattern that reads the parameter cannot see a body
            // declaration at all, so the parameter must stay.
            Expr::Lit(lit)
                if !preserve_arg_list
                    && !eval_observes_mapped_arguments
                    && !binding_uses.has_declaration(&param_binding)
                    && !param_value_refs.refs.contains(&param_binding) =>
            {
                let kind = if binding_uses.has_direct_write(&param_binding) {
                    VarDeclKind::Let
                } else {
                    VarDeclKind::Const
                };
                plan.literal_inserts
                    .push((i, param_sym, param_ctxt, lit.clone(), kind));
            }
            _ => {}
        }
    }

    plan
}

/// Rename every identifier of a planned parameter rename by `(sym, ctxt)`:
/// the parameter binding itself, its reads in the body, and its reads inside
/// sibling parameter defaults and patterns (`(e, t, r = t) => …`). Only the
/// binding name changes; shorthand keys stay intact and the ctxt is kept.
fn apply_rename_rewrites<P>(params: &mut P, body: &mut FunctionBody, plan: &RewritePlan)
where
    P: VisitMutWith<BindingRenamer>,
{
    let renames: Vec<BindingRename> = plan
        .renames
        .iter()
        .map(|(_, old, new, ctxt)| BindingRename {
            old: (old.clone(), *ctxt),
            new: new.clone(),
        })
        .collect();
    rename_bindings(params, &renames);
    rename_bindings(body, &renames);
}

/// Bindings read inside the parameter list itself: default expressions and
/// computed pattern keys. Binding positions are declarations, not reads.
fn collect_param_value_refs<P>(params: &P) -> ParamValueRefs
where
    P: VisitWith<ParamValueRefs>,
{
    let mut collector = ParamValueRefs {
        refs: HashSet::default(),
        names: HashSet::default(),
    };
    params.visit_with(&mut collector);
    collector
}

struct ParamValueRefs {
    refs: HashSet<(Atom, SyntaxContext)>,
    names: HashSet<Atom>,
}

impl Visit for ParamValueRefs {
    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        self.names.insert(binding.id.sym.clone());
    }

    fn visit_ident(&mut self, ident: &Ident) {
        self.names.insert(ident.sym.clone());
        self.refs.insert((ident.sym.clone(), ident.ctxt));
    }
}

fn prepend_literal_decls(
    body: &mut FunctionBody,
    literal_inserts: &[(usize, Atom, SyntaxContext, Lit, VarDeclKind)],
) {
    if literal_inserts.is_empty() {
        return;
    }
    // Sort by ascending original index to preserve declaration order.
    let mut sorted: Vec<&(usize, Atom, SyntaxContext, Lit, VarDeclKind)> =
        literal_inserts.iter().collect();
    sorted.sort_by_key(|t| t.0);
    let literal_stmts: Vec<Stmt> = sorted
        .into_iter()
        .map(|(_, sym, ctxt, lit, kind)| make_literal_decl(sym.clone(), *ctxt, lit.clone(), *kind))
        .collect();
    let old_body = std::mem::take(&mut body.stmts);
    body.stmts = literal_stmts;
    body.stmts.extend(old_body);
}

/// Pick a name that doesn't collide with anything in `taken`. Returns
/// `preferred` when free; otherwise appends `_1`, `_2`, ... until unique.
fn pick_non_conflicting_name(preferred: &Atom, taken: &HashSet<Atom>) -> Atom {
    if !taken.contains(preferred) {
        return preferred.clone();
    }
    for suffix in 1usize.. {
        let candidate: Atom = format!("{}_{}", preferred, suffix).into();
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Collect every binding name introduced anywhere inside the body, including
/// nested function/arrow params and bodies, catch params, etc. Used to decide
/// whether substituting an outer ident into the body could be shadowed by an
/// inner binding with the same name.
fn collect_all_binding_names(body: &FunctionBody) -> HashSet<Atom> {
    struct Collector {
        names: HashSet<Atom>,
    }

    fn collect_pat(pat: &Pat, names: &mut HashSet<Atom>) {
        match pat {
            Pat::Ident(b) => {
                names.insert(b.id.sym.clone());
            }
            Pat::Array(a) => {
                for elem in a.elems.iter().flatten() {
                    collect_pat(elem, names);
                }
            }
            Pat::Object(o) => {
                for prop in &o.props {
                    match prop {
                        ObjectPatProp::Assign(a) => {
                            names.insert(a.key.sym.clone());
                        }
                        ObjectPatProp::KeyValue(kv) => collect_pat(&kv.value, names),
                        ObjectPatProp::Rest(r) => collect_pat(&r.arg, names),
                    }
                }
            }
            Pat::Rest(r) => collect_pat(&r.arg, names),
            Pat::Assign(a) => collect_pat(&a.left, names),
            _ => {}
        }
    }

    impl Visit for Collector {
        fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
            collect_pat(&decl.name, &mut self.names);
            decl.visit_children_with(self);
        }
        fn visit_fn_decl(&mut self, decl: &FnDecl) {
            self.names.insert(decl.ident.sym.clone());
            decl.visit_children_with(self);
        }
        fn visit_class_decl(&mut self, decl: &ClassDecl) {
            self.names.insert(decl.ident.sym.clone());
            decl.visit_children_with(self);
        }
        fn visit_function(&mut self, f: &Function) {
            for p in &f.params {
                collect_pat(&p.pat, &mut self.names);
            }
            f.visit_children_with(self);
        }
        fn visit_arrow_expr(&mut self, a: &ArrowExpr) {
            for p in &a.params {
                collect_pat(p, &mut self.names);
            }
            a.visit_children_with(self);
        }
        fn visit_constructor(&mut self, c: &Constructor) {
            for p in &c.params {
                if let ParamOrTsParamProp::Param(p) = p {
                    collect_pat(&p.pat, &mut self.names);
                }
            }
            c.visit_children_with(self);
        }
        fn visit_catch_clause(&mut self, c: &CatchClause) {
            if let Some(p) = &c.param {
                collect_pat(p, &mut self.names);
            }
            c.visit_children_with(self);
        }
    }

    let mut c = Collector {
        names: HashSet::default(),
    };
    body.visit_with(&mut c);
    c.names
}

fn make_literal_decl(name: Atom, binding_ctxt: SyntaxContext, lit: Lit, kind: VarDeclKind) -> Stmt {
    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: Ident {
                    span: DUMMY_SP,
                    ctxt: binding_ctxt,
                    sym: name,
                    optional: false,
                },
                type_ann: None,
            }),
            init: Some(Box::new(Expr::Lit(lit))),
            definite: false,
        }],
    })))
}

fn body_uses_own_arguments(body: &FunctionBody) -> bool {
    struct Checker {
        found: bool,
    }

    impl Visit for Checker {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym.as_ref() == "arguments" {
                self.found = true;
            }
        }

        // Nested non-arrow functions have their own `arguments` binding. Arrow
        // functions intentionally keep the default traversal because they
        // capture the enclosing function's `arguments`.
        fn visit_function(&mut self, _: &Function) {}
        fn visit_constructor(&mut self, _: &Constructor) {}
        fn visit_method_prop(&mut self, _: &MethodProp) {}
        fn visit_getter_prop(&mut self, _: &GetterProp) {}
        fn visit_setter_prop(&mut self, _: &SetterProp) {}
    }

    let mut checker = Checker { found: false };
    body.visit_with(&mut checker);
    checker.found
}

fn body_contains_object_create(body: &FunctionBody) -> bool {
    struct Finder {
        found: bool,
    }

    impl Visit for Finder {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if is_object_create_call(call) {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder { found: false };
    body.visit_with(&mut finder);
    finder.found
}

fn is_object_create_call(call: &CallExpr) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };

    obj.sym.as_ref() == "Object"
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "create")
}
