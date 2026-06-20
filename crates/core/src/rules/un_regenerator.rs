use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, ArrowExpr, AssignExpr, AssignOp, AssignTarget, AwaitExpr, BlockStmt, BlockStmtOrExpr,
    BreakStmt, CallExpr, Callee, ContinueStmt, Decl, Expr, ExprOrSpread, ExprStmt, FnDecl, FnExpr,
    ForStmt, Function, Ident, ImportSpecifier, Lit, MemberExpr, MemberProp, Module, ModuleDecl,
    ModuleItem, Number, Param, ParenExpr, Pat, ReturnStmt, SimpleAssignTarget, Stmt, SwitchCase,
    VarDeclarator, WhileStmt, YieldExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::facts::{HelperKind, ModuleFactsMap};

use super::helper_matcher::{
    binding_key, count_binding_refs, member_prop_name, remove_fn_decls_by_binding,
    remove_var_declarators_by_binding,
};
use super::state_machine::{
    invert_condition, jump_target_stmt, return_jump_target, stmts_contain_state_opcode_return,
    OpcodeReturnScan, StateMachineProgram,
};
use super::transpiler_helper_utils::{
    BindingKey, LocalHelperContext, TranspilerHelperKind, TsHelperKind,
};
use super::un_async_await::try_transform_ts_generator_body;

use crate::js_names::is_likely_generated_alias;
use crate::utils::paren::strip_parens;

pub struct UnRegenerator<'a> {
    unresolved_mark: Mark,
    module_facts: Option<&'a ModuleFactsMap>,
}

impl UnRegenerator<'_> {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self {
            unresolved_mark,
            module_facts: None,
        }
    }
}

impl<'a> UnRegenerator<'a> {
    pub fn new_with_facts(unresolved_mark: Mark, module_facts: &'a ModuleFactsMap) -> Self {
        Self {
            unresolved_mark,
            module_facts: Some(module_facts),
        }
    }

    pub(crate) fn run_with_helpers(
        module: &mut Module,
        unresolved_mark: Mark,
        module_facts: Option<&ModuleFactsMap>,
        local_helpers: &LocalHelperContext,
    ) {
        run_un_regenerator(module, unresolved_mark, module_facts, local_helpers);
    }
}

impl VisitMut for UnRegenerator<'_> {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect_with_mark(module, self.unresolved_mark);
        run_un_regenerator(
            module,
            self.unresolved_mark,
            self.module_facts,
            &local_helpers,
        );
    }
}

fn run_un_regenerator(
    module: &mut Module,
    unresolved_mark: Mark,
    module_facts: Option<&ModuleFactsMap>,
    local_helpers: &LocalHelperContext,
) {
    // Phase 1: Detect _asyncToGenerator helper bindings (scope-aware)
    let helpers = local_helpers.helpers();
    let mut async_to_gen_bindings: Vec<BindingKey> = helpers
        .iter()
        .filter(|(_, kind)| **kind == TranspilerHelperKind::AsyncToGenerator)
        .map(|((sym, ctxt), _)| (sym.clone(), *ctxt))
        .collect();
    let mut async_to_gen_default_members = Vec::new();
    if let Some(module_facts) = module_facts {
        let imported_helpers =
            collect_cross_module_async_helpers(module, module_facts, unresolved_mark);
        async_to_gen_bindings.extend(imported_helpers.direct);
        async_to_gen_default_members.extend(imported_helpers.default_members);
    }
    let async_to_gen_callees = AsyncToGenCallees {
        direct: &async_to_gen_bindings,
        default_members: &async_to_gen_default_members,
    };
    let generator_helpers: Vec<BindingKey> = local_helpers
        .ts_helpers_of_kind(TsHelperKind::Generator)
        .into_iter()
        .collect();
    let esbuild_async_helpers = collect_esbuild_async_helpers(module, unresolved_mark);
    let esbuild_yield_star_helpers = collect_esbuild_yield_star_helpers(module);

    // Phase 2: Transform functions containing regeneratorRuntime.wrap()
    // and _asyncToGenerator() calls. Track consumed mark bindings.
    let mut consumed_marks: Vec<BindingKey> = Vec::new();

    transform_babel_async_trampolines(
        module,
        &async_to_gen_callees,
        &generator_helpers,
        &mut consumed_marks,
    );

    let mut transformer = FunctionTransformer {
        unresolved_mark,
        async_to_gen_callees: &async_to_gen_callees,
        generator_helpers: &generator_helpers,
        esbuild_async_helpers: &esbuild_async_helpers,
        esbuild_yield_star_helpers: &esbuild_yield_star_helpers,
        consumed_marks: &mut consumed_marks,
    };
    module.visit_mut_with(&mut transformer);
    collapse_async_trampoline_iifes(module);
    collapse_async_trampoline_sequences(module);
    collapse_async_trampoline_assignments(module);

    // Post-pass: strip the `_regeneratorValues` iterator wrapper from recovered
    // `yield*` delegations. Decode already handles the canonical name, but
    // top-level mangling renames the helper, so match the detected binding too.
    let regenerator_values_helpers = collect_regenerator_values_helpers(module);
    if !regenerator_values_helpers.is_empty() {
        let mut unwrapper = RegeneratorValuesUnwrapper {
            helpers: &regenerator_values_helpers,
        };
        module.visit_mut_with(&mut unwrapper);
        remove_unused_helper_decls(module, &regenerator_values_helpers);
    }

    // Phase 3: Remove only the mark declarations that were consumed
    remove_consumed_mark_declarations(module, &consumed_marks);

    // Phase 4: Remove _asyncToGenerator helper if no longer referenced
    if !async_to_gen_bindings.is_empty() {
        let roots: HashMap<BindingKey, TranspilerHelperKind> = helpers
            .iter()
            .filter(|(key, kind)| {
                **kind == TranspilerHelperKind::AsyncToGenerator
                    && async_to_gen_bindings.contains(key)
            })
            .map(|(key, kind)| (key.clone(), *kind))
            .collect();
        local_helpers.remove_helpers_with_dependencies(module, roots);
    }

    remove_unused_helper_decls(module, &esbuild_async_helpers);
    remove_unused_helper_decls(module, &esbuild_yield_star_helpers);
}

struct AsyncToGenCallees<'a> {
    direct: &'a [BindingKey],
    default_members: &'a [BindingKey],
}

#[derive(Default)]
struct CrossModuleAsyncHelpers {
    direct: Vec<BindingKey>,
    default_members: Vec<BindingKey>,
}

fn collect_cross_module_async_helpers(
    module: &Module,
    module_facts: &ModuleFactsMap,
    unresolved_mark: Mark,
) -> CrossModuleAsyncHelpers {
    let mut helpers = CrossModuleAsyncHelpers::default();

    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                if !module_exports_helper(
                    module_facts,
                    &str_to_atom(&import.src.value),
                    "default",
                    HelperKind::AsyncToGenerator,
                ) {
                    continue;
                }

                for specifier in &import.specifiers {
                    match specifier {
                        ImportSpecifier::Default(default) => {
                            helpers
                                .direct
                                .push((default.local.sym.clone(), default.local.ctxt));
                        }
                        ImportSpecifier::Namespace(namespace) => {
                            helpers
                                .default_members
                                .push((namespace.local.sym.clone(), namespace.local.ctxt));
                        }
                        ImportSpecifier::Named(named)
                            if named
                                .imported
                                .as_ref()
                                .is_some_and(|imported| export_name_is(imported, "default")) =>
                        {
                            helpers
                                .direct
                                .push((named.local.sym.clone(), named.local.ctxt));
                        }
                        _ => {}
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    let Pat::Ident(binding) = &decl.name else {
                        continue;
                    };
                    let Some(init) = &decl.init else {
                        continue;
                    };
                    let Some(source) = require_source_from_interop_init(init, unresolved_mark)
                    else {
                        continue;
                    };
                    if module_exports_helper(
                        module_facts,
                        &source,
                        "default",
                        HelperKind::AsyncToGenerator,
                    ) {
                        helpers
                            .default_members
                            .push((binding.id.sym.clone(), binding.id.ctxt));
                    }
                }
            }
            _ => {}
        }
    }

    helpers
}

fn module_exports_helper(
    module_facts: &ModuleFactsMap,
    source: &Atom,
    exported: &str,
    kind: HelperKind,
) -> bool {
    module_facts.get(source.as_ref()).is_some_and(|facts| {
        facts
            .helper_exports
            .iter()
            .any(|helper| helper.exported.as_ref() == exported && helper.kind == kind)
    })
}

fn require_source_from_interop_init(expr: &Expr, unresolved_mark: Mark) -> Option<Atom> {
    if let Some(source) = require_source(expr, unresolved_mark) {
        return Some(source);
    }

    let Expr::Call(call) = expr else {
        return None;
    };
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    require_source(&call.args[0].expr, unresolved_mark)
}

fn require_source(expr: &Expr, unresolved_mark: Mark) -> Option<Atom> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    let callee = call.callee.as_expr()?;
    if !matches!(callee.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "require" && id.ctxt.outer() == unresolved_mark)
    {
        return None;
    }
    let Expr::Lit(Lit::Str(source)) = call.args[0].expr.as_ref() else {
        return None;
    };
    Some(str_to_atom(&source.value))
}

fn transform_babel_async_trampolines(
    module: &mut Module,
    async_to_gen_callees: &AsyncToGenCallees,
    generator_helpers: &[BindingKey],
    consumed_marks: &mut Vec<BindingKey>,
) {
    let mut index = 0;
    while index + 1 < module.body.len() {
        let Some((public_name, private_key)) = extract_public_trampoline_fn(&module.body[index])
        else {
            index += 1;
            continue;
        };

        if binding_used_outside_pair(&module.body, index, index + 1, &private_key) {
            index += 1;
            continue;
        }

        let Some((params, mut stmts, mark_key)) = extract_private_trampoline_body(
            &module.body[index + 1],
            &private_key,
            async_to_gen_callees,
            generator_helpers,
        ) else {
            index += 1;
            continue;
        };

        let Some(public_decl) = public_fn_decl_from_module_item_mut(&mut module.body[index]) else {
            index += 1;
            continue;
        };
        if public_decl.ident.sym != public_name.0 || public_decl.ident.ctxt != public_name.1 {
            index += 1;
            continue;
        }

        replace_yield_with_await(&mut stmts);
        public_decl.function.params = params;
        public_decl.function.body = Some(BlockStmt {
            span: DUMMY_SP,
            ctxt: Default::default(),
            stmts,
        });
        public_decl.function.is_async = true;
        public_decl.function.is_generator = false;

        if let Some(mark_key) = mark_key {
            consumed_marks.push(mark_key);
        }

        module.body.remove(index + 1);
        index += 1;
    }
}

fn extract_public_trampoline_fn(item: &ModuleItem) -> Option<(BindingKey, BindingKey)> {
    let fn_decl = public_fn_decl_from_module_item(item)?;
    let private_ident = private_apply_return_ident(fn_decl)?;
    Some((
        (fn_decl.ident.sym.clone(), fn_decl.ident.ctxt),
        (private_ident.sym.clone(), private_ident.ctxt),
    ))
}

fn public_fn_decl_from_module_item(item: &ModuleItem) -> Option<&FnDecl> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => Some(fn_decl),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
            Decl::Fn(fn_decl) => Some(fn_decl),
            _ => None,
        },
        _ => None,
    }
}

fn public_fn_decl_from_module_item_mut(item: &mut ModuleItem) -> Option<&mut FnDecl> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => Some(fn_decl),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &mut export.decl {
            Decl::Fn(fn_decl) => Some(fn_decl),
            _ => None,
        },
        _ => None,
    }
}

fn private_apply_return_ident(fn_decl: &FnDecl) -> Option<Ident> {
    let body = fn_decl.function.body.as_ref()?;
    if body.stmts.len() != 1 {
        return None;
    }
    let Stmt::Return(ret) = &body.stmts[0] else {
        return None;
    };
    let arg = ret.arg.as_deref()?;
    extract_apply_this_arguments_callee(arg)
}

fn extract_apply_this_arguments_callee(expr: &Expr) -> Option<Ident> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if call.args.len() != 2 {
        return None;
    }
    if !matches!(call.args[0].expr.as_ref(), Expr::This(_)) {
        return None;
    }
    if !matches!(call.args[1].expr.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "arguments") {
        return None;
    }

    let callee = call.callee.as_expr()?;
    let Expr::Member(apply_member) = callee.as_ref() else {
        return None;
    };
    if !member_prop_name(&apply_member.prop, "apply") {
        return None;
    }
    let Expr::Ident(id) = apply_member.obj.as_ref() else {
        return None;
    };
    Some(id.clone())
}

fn extract_private_trampoline_body(
    item: &ModuleItem,
    private_key: &BindingKey,
    async_to_gen_callees: &AsyncToGenCallees,
    generator_helpers: &[BindingKey],
) -> Option<(Vec<Param>, Vec<Stmt>, Option<BindingKey>)> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) = item else {
        return None;
    };
    if fn_decl.ident.sym != private_key.0 || fn_decl.ident.ctxt != private_key.1 {
        return None;
    }
    let body = fn_decl.function.body.as_ref()?;
    let gen_arg = match body.stmts.as_slice() {
        [assign_stmt, return_stmt]
            if apply_return_ident_from_stmt(return_stmt)
                .is_some_and(|id| id.sym == private_key.0 && id.ctxt == private_key.1) =>
        {
            extract_async_assignment_arg(assign_stmt, private_key, async_to_gen_callees)?
        }
        [return_stmt] => extract_async_assignment_arg_from_returned_apply(
            return_stmt,
            private_key,
            async_to_gen_callees,
        )?,
        _ => return None,
    };
    extract_async_to_gen_body_with_params(gen_arg, generator_helpers)
}

fn apply_return_ident_from_stmt(stmt: &Stmt) -> Option<Ident> {
    let Stmt::Return(ret) = stmt else {
        return None;
    };
    let arg = ret.arg.as_deref()?;
    extract_apply_this_arguments_callee(arg)
}

fn extract_async_assignment_arg(
    stmt: &Stmt,
    private_key: &BindingKey,
    async_to_gen_callees: &AsyncToGenCallees,
) -> Option<Expr> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left else {
        return None;
    };
    if left.id.sym != private_key.0 || left.id.ctxt != private_key.1 {
        return None;
    }

    let Expr::Call(call) = assign.right.as_ref() else {
        return None;
    };
    if call.args.len() != 1 {
        return None;
    }
    let callee = call.callee.as_expr()?;
    if !is_async_to_gen_callee(callee, async_to_gen_callees) {
        return None;
    }

    Some(*call.args[0].expr.clone())
}

fn extract_async_assignment_arg_from_returned_apply(
    stmt: &Stmt,
    private_key: &BindingKey,
    async_to_gen_callees: &AsyncToGenCallees,
) -> Option<Expr> {
    let Stmt::Return(ret) = stmt else {
        return None;
    };
    let Expr::Call(apply_call) = ret.arg.as_deref()? else {
        return None;
    };
    if apply_call.args.len() != 2 {
        return None;
    }
    if !matches!(apply_call.args[0].expr.as_ref(), Expr::This(_)) {
        return None;
    }
    if !matches!(apply_call.args[1].expr.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "arguments")
    {
        return None;
    }

    let apply_callee = apply_call.callee.as_expr()?;
    let Expr::Member(apply_member) = apply_callee.as_ref() else {
        return None;
    };
    if !member_prop_name(&apply_member.prop, "apply") {
        return None;
    }

    let Expr::Assign(assign) = strip_parens(&apply_member.obj) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left else {
        return None;
    };
    if left.id.sym != private_key.0 || left.id.ctxt != private_key.1 {
        return None;
    }

    let Expr::Call(helper_call) = assign.right.as_ref() else {
        return None;
    };
    if helper_call.args.len() != 1 {
        return None;
    }
    let helper_callee = helper_call.callee.as_expr()?;
    if !is_async_to_gen_callee(helper_callee, async_to_gen_callees) {
        return None;
    }

    Some(*helper_call.args[0].expr.clone())
}

fn extract_async_to_gen_body_with_params(
    gen_fn_arg: Expr,
    generator_helpers: &[BindingKey],
) -> Option<(Vec<Param>, Vec<Stmt>, Option<BindingKey>)> {
    match gen_fn_arg {
        Expr::Fn(fn_expr) => {
            let params = fn_expr.function.params.clone();
            if fn_expr.function.is_generator {
                return Some((params, fn_expr.function.body?.stmts, None));
            }
            let mut body = fn_expr.function.body?;
            let mark_key = if let Some(mark_key) = try_transform_regenerator_wrap(&mut body) {
                mark_key
            } else if try_transform_ts_generator_body(&mut body, generator_helpers) {
                None
            } else {
                return None;
            };
            Some((params, body.stmts, mark_key))
        }
        Expr::Call(mark_call) => {
            let callee_expr = mark_call.callee.as_expr()?;
            let Expr::Member(member) = callee_expr.as_ref() else {
                return None;
            };
            if !is_mark_prop(&member.prop) || mark_call.args.len() != 1 {
                return None;
            }
            let Expr::Fn(fn_expr) = *mark_call.args.into_iter().next()?.expr else {
                return None;
            };
            let params = fn_expr.function.params.clone();
            let mut body = fn_expr.function.body?;
            let mark_key = if let Some(mark_key) = try_transform_regenerator_wrap(&mut body) {
                mark_key
            } else if try_transform_ts_generator_body(&mut body, generator_helpers) {
                None
            } else {
                return None;
            };
            Some((params, body.stmts, mark_key))
        }
        _ => None,
    }
}

fn binding_used_outside_pair(
    items: &[ModuleItem],
    first: usize,
    second: usize,
    key: &BindingKey,
) -> bool {
    items.iter().enumerate().any(|(index, item)| {
        if index == first || index == second {
            return false;
        }
        let mut finder = BindingUseFinder {
            key: key.clone(),
            found: false,
        };
        item.visit_with(&mut finder);
        finder.found
    })
}

struct BindingUseFinder {
    key: BindingKey,
    found: bool,
}

impl Visit for BindingUseFinder {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        if let Some(init) = &decl.init {
            init.visit_with(self);
        }
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == self.key.0 && ident.ctxt == self.key.1 {
            self.found = true;
        }
    }
}

struct FunctionTransformer<'a> {
    unresolved_mark: Mark,
    async_to_gen_callees: &'a AsyncToGenCallees<'a>,
    generator_helpers: &'a [BindingKey],
    esbuild_async_helpers: &'a [BindingKey],
    esbuild_yield_star_helpers: &'a [BindingKey],
    consumed_marks: &'a mut Vec<BindingKey>,
}

impl VisitMut for FunctionTransformer<'_> {
    fn visit_mut_assign_expr(&mut self, assign: &mut AssignExpr) {
        assign.visit_mut_children_with(self);
        if let Some((expr, mark_key)) = try_transform_async_to_generator_expr(
            *assign.right.clone(),
            self.async_to_gen_callees,
            self.generator_helpers,
        ) {
            *assign.right = expr;
            if let Some(mark_key) = mark_key {
                self.consumed_marks.push(mark_key);
            }
        }
    }

    fn visit_mut_var_declarator(&mut self, decl: &mut VarDeclarator) {
        decl.visit_mut_children_with(self);
        let Some(init) = decl.init.take() else {
            return;
        };
        if let Some((expr, mark_key)) = try_transform_async_to_generator_expr(
            *init.clone(),
            self.async_to_gen_callees,
            self.generator_helpers,
        ) {
            decl.init = Some(Box::new(expr));
            if let Some(mark_key) = mark_key {
                self.consumed_marks.push(mark_key);
            }
        } else if let Some(expr) = try_collapse_async_trampoline_iife(&init) {
            decl.init = Some(Box::new(expr));
        } else {
            decl.init = Some(init);
        }
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        if let Some(body) = func.body.as_mut() {
            if try_transform_esbuild_async_function(body, self.esbuild_async_helpers) {
                func.is_async = true;
                func.visit_mut_children_with(self);
                return;
            }
        }

        // Try _asyncToGenerator BEFORE recursing — the inner function hasn't
        // been transformed yet, so we can still detect the full pattern.
        if let Some(body) = func.body.as_mut() {
            if try_transform_async_to_generator(
                body,
                self.async_to_gen_callees,
                self.generator_helpers,
                self.unresolved_mark,
            ) {
                func.is_async = true;
                // Still recurse into the (now-rewritten) body for nested cases
                func.visit_mut_children_with(self);
                return;
            }
        }

        func.visit_mut_children_with(self);

        let body = match func.body.as_mut() {
            Some(b) => b,
            None => return,
        };

        // Try regeneratorRuntime.wrap() transform
        if let Some(mark_key) = try_transform_regenerator_wrap(body) {
            func.is_generator = true;
            if let Some(key) = mark_key {
                self.consumed_marks.push(key);
            }
        }
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        if try_transform_esbuild_async_arrow(arrow, self.esbuild_async_helpers) {
            arrow.visit_mut_children_with(self);
            return;
        }

        arrow.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Call(call) = expr {
            // _asyncToGenerator(fn)() IIFE
            if call.args.is_empty() {
                if let Callee::Expr(callee_expr) = &mut call.callee {
                    if is_paramless_async_to_gen_iife(callee_expr, self.async_to_gen_callees) {
                        if let Some((transformed, mark_key)) = try_transform_async_to_generator_expr(
                            callee_expr.as_ref().clone(),
                            self.async_to_gen_callees,
                            self.generator_helpers,
                        ) {
                            **callee_expr = Expr::Paren(ParenExpr {
                                span: DUMMY_SP,
                                expr: Box::new(transformed),
                            });
                            if let Some(mark_key) = mark_key {
                                self.consumed_marks.push(mark_key);
                            }
                        }
                        return;
                    }
                }
            }

            // __async(this, null, fn*(){…}) standalone expression
            if let Some(mut stmts) =
                extract_esbuild_async_call_body(expr, self.esbuild_async_helpers)
            {
                replace_yield_with_await(&mut stmts);
                *expr = Expr::Call(CallExpr {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    callee: Callee::Expr(Box::new(Expr::Paren(ParenExpr {
                        span: DUMMY_SP,
                        expr: Box::new(Expr::Fn(FnExpr {
                            ident: None,
                            function: Box::new(Function {
                                params: vec![],
                                decorators: vec![],
                                span: DUMMY_SP,
                                ctxt: Default::default(),
                                body: Some(BlockStmt {
                                    span: DUMMY_SP,
                                    ctxt: Default::default(),
                                    stmts,
                                }),
                                is_generator: false,
                                is_async: true,
                                type_params: None,
                                return_type: None,
                            }),
                        })),
                    }))),
                    args: vec![],
                    type_args: None,
                });
            }
        }
    }

    fn visit_mut_yield_expr(&mut self, yield_expr: &mut YieldExpr) {
        yield_expr.visit_mut_children_with(self);

        if !yield_expr.delegate {
            return;
        }

        let Some(arg) = yield_expr.arg.take() else {
            return;
        };
        if let Some(unwrapped) =
            unwrap_esbuild_yield_star_arg(&arg, self.esbuild_yield_star_helpers)
        {
            yield_expr.arg = Some(unwrapped);
        } else {
            yield_expr.arg = Some(arg);
        }
    }
}

// ============================================================
// regeneratorRuntime.wrap() → function*
// ============================================================

/// Returns the consumed mark binding key (sym + ctxt) on success.
fn try_transform_regenerator_wrap(body: &mut BlockStmt) -> Option<Option<BindingKey>> {
    let return_idx = body.stmts.iter().position(is_regenerator_wrap_return)?;

    // P1-1: Pre-check for nested control flow before extracting.
    if has_nested_control_flow_in_stmt(&body.stmts[return_idx]) {
        return None;
    }

    // Extract the mark binding key (2nd arg to .wrap()) before consuming
    let mark_name = extract_wrap_mark_key(&body.stmts[return_idx]);

    let ret_stmt = body.stmts[return_idx].clone();
    let (state_name, cases, try_regions) = extract_wrap_args(ret_stmt)?;

    let new_stmts = decode_babel_state_machine(&state_name, cases, try_regions);
    // Safety net: if a forward conditional jump could not be structured, an
    // opcode goto (`return [3, N]`) leaks into the output. Rather than emit
    // broken control flow, leave the function un-recovered.
    if stmts_contain_state_opcode_return(&new_stmts, OpcodeReturnScan::IncludeNestedFunctions) {
        return None;
    }
    body.stmts.remove(return_idx);
    body.stmts.splice(return_idx..return_idx, new_stmts);
    Some(mark_name)
}

/// Extract the mark binding key (sym + ctxt) from the 2nd argument of .wrap(fn, markIdent, ...)
fn extract_wrap_mark_key(stmt: &Stmt) -> Option<BindingKey> {
    let Stmt::Return(ret) = stmt else { return None };
    let arg = ret.arg.as_ref()?;
    let Expr::Call(call) = arg.as_ref() else {
        return None;
    };
    if call.args.len() < 2 {
        return None;
    }
    let Expr::Ident(id) = call.args[1].expr.as_ref() else {
        return None;
    };
    Some((id.sym.clone(), id.ctxt))
}

/// Check if the regenerator.wrap() state machine contains nested control flow
/// (if/else blocks with state transitions) that we can't safely linearize.
fn has_nested_control_flow_in_stmt(stmt: &Stmt) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = &ret.arg else { return false };
    let Expr::Call(call) = arg.as_ref() else {
        return false;
    };
    if call.args.is_empty() {
        return false;
    }
    let wrap_try_regions = extract_wrap_try_regions_from_call(call);
    let fn_expr = &call.args[0].expr;
    let cases = match fn_expr.as_ref() {
        Expr::Fn(f) => {
            let param_name = match f.function.params.first().map(|p| &p.pat) {
                Some(Pat::Ident(bi)) => bi.id.sym.clone(),
                _ => return false,
            };
            let Some(body) = &f.function.body else {
                return false;
            };
            extract_switch_cases_ref(body).map(|c| (param_name, c))
        }
        Expr::Arrow(a) => {
            let param_name = match a.params.first() {
                Some(Pat::Ident(bi)) => bi.id.sym.clone(),
                _ => return false,
            };
            match a.body.as_ref() {
                swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) => {
                    extract_switch_cases_ref(body).map(|c| (param_name, c))
                }
                _ => None,
            }
        }
        _ => None,
    };
    let Some((state_name, cases)) = cases else {
        return false;
    };
    // Check each case's top-level statements for nested blocks that
    // contain state machine operations (_ctx.next or break). A supported
    // forward state-jump (`if (cond) { _ctx.next = N; break; }`) is decoded
    // structurally, so it does not count as unsupported control flow.
    for case in cases {
        for stmt in &case.cons {
            if has_state_ops_in_nested_block(&state_name, stmt)
                && !is_supported_nested_state_jump(&state_name, stmt)
            {
                return true;
            }
        }
    }
    // `_ctx.catch(...)` is only safe when Babel also supplied the try-region list
    // as the 4th .wrap() argument.
    if wrap_try_regions.is_empty() {
        for case in cases {
            if case_uses_catch(&state_name, case) {
                return true;
            }
        }
    }
    false
}

fn is_supported_nested_state_jump(state_name: &Atom, stmt: &Stmt) -> bool {
    let Stmt::If(if_stmt) = stmt else {
        return false;
    };
    if if_stmt.alt.is_some() {
        return false;
    }
    extract_next_break_target(state_name, &if_stmt.cons).is_some()
}

/// Check if a statement contains _ctx.next assignments or break statements
/// inside nested blocks (if/else, block statements, etc.) — not at the top level.
fn has_state_ops_in_nested_block(state_name: &Atom, stmt: &Stmt) -> bool {
    match stmt {
        Stmt::If(if_stmt) => {
            has_state_ops_deep(state_name, &if_stmt.cons)
                || if_stmt
                    .alt
                    .as_ref()
                    .is_some_and(|alt| has_state_ops_deep(state_name, alt))
        }
        Stmt::Block(block) => block
            .stmts
            .iter()
            .any(|s| has_state_ops_deep(state_name, s)),
        _ => false,
    }
}

fn has_state_ops_deep(state_name: &Atom, stmt: &Stmt) -> bool {
    struct Finder {
        state_name: Atom,
        found: bool,
    }
    impl swc_core::ecma::visit::Visit for Finder {
        fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
            if let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) {
                if is_ident_with_name(&left_member.obj, &self.state_name)
                    && is_next_prop(&left_member.prop)
                {
                    self.found = true;
                    return;
                }
            }
            assign.visit_children_with(self);
        }
        fn visit_break_stmt(&mut self, _: &swc_core::ecma::ast::BreakStmt) {
            self.found = true;
        }
    }
    let mut f = Finder {
        state_name: state_name.clone(),
        found: false,
    };
    stmt.visit_with(&mut f);
    f.found
}

/// Check if a switch case contains `_ctx.catch(...)` calls — signals try/catch.
fn case_uses_catch(state_name: &Atom, case: &SwitchCase) -> bool {
    struct CatchFinder {
        state_name: Atom,
        found: bool,
    }
    impl swc_core::ecma::visit::Visit for CatchFinder {
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if let Some(callee) = call.callee.as_expr() {
                if let Expr::Member(member) = callee.as_ref() {
                    if is_ident_with_name(&member.obj, &self.state_name)
                        && member_prop_name(&member.prop, "catch")
                    {
                        self.found = true;
                        return;
                    }
                }
            }
            call.visit_children_with(self);
        }
    }
    let mut f = CatchFinder {
        state_name: state_name.clone(),
        found: false,
    };
    for stmt in &case.cons {
        stmt.visit_with(&mut f);
        if f.found {
            return true;
        }
    }
    false
}

fn extract_switch_cases_ref(body: &BlockStmt) -> Option<&[SwitchCase]> {
    for stmt in &body.stmts {
        match stmt {
            Stmt::While(while_stmt) => {
                if let Some(cases) = switch_cases_from_loop_body_ref(while_stmt.body.as_ref()) {
                    return Some(cases);
                }
            }
            Stmt::For(for_stmt) => {
                if let Some(cases) = switch_cases_from_loop_body_ref(for_stmt.body.as_ref()) {
                    return Some(cases);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_regenerator_wrap_return(stmt: &Stmt) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = &ret.arg else { return false };
    is_wrap_call(arg)
}

/// Check if expr is `<something>.wrap(stateMachineFn, ...)` or Babel 7.27+
/// `<something>.w(stateMachineFn, ...)`.
/// where stateMachineFn contains the distinctive `while(true) { switch(param.prev = param.next) }` pattern.
fn is_wrap_call(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Some(callee_expr) = call.callee.as_expr() else {
        return false;
    };
    let Expr::Member(member) = callee_expr.as_ref() else {
        return false;
    };
    if !is_wrap_prop(&member.prop) {
        return false;
    }
    if call.args.is_empty() {
        return false;
    }
    // Validate that the first argument is a state machine function
    is_state_machine_fn(&call.args[0].expr)
}

/// Check if an expression is a state machine function:
/// `function(param) { while(true) { switch(param.prev = param.next) { ... } } }`
/// or arrow: `(param) => { while(true) { switch(...) { ... } } }`
fn is_state_machine_fn(expr: &Expr) -> bool {
    match expr {
        Expr::Fn(fn_expr) => {
            if fn_expr.function.params.len() != 1 {
                return false;
            }
            let param_name = match &fn_expr.function.params[0].pat {
                Pat::Ident(bi) => &bi.id.sym,
                _ => return false,
            };
            let Some(body) = &fn_expr.function.body else {
                return false;
            };
            has_state_machine_structure(body, param_name)
        }
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 1 {
                return false;
            }
            let param_name = match &arrow.params[0] {
                Pat::Ident(bi) => &bi.id.sym,
                _ => return false,
            };
            match arrow.body.as_ref() {
                swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) => {
                    has_state_machine_structure(body, param_name)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// Check for `while(true) { switch(param.prev = param.next) { ... case "end": ... } }`
/// or `for(;;) { switch(...) { ... } }`
fn has_state_machine_structure(body: &BlockStmt, param_name: &Atom) -> bool {
    // Look for a while(true) or for(;;) loop containing the switch
    for stmt in &body.stmts {
        match stmt {
            Stmt::While(while_stmt) => {
                if !is_true_expr(&while_stmt.test) {
                    continue;
                }
                if loop_body_has_state_switch(while_stmt.body.as_ref(), param_name) {
                    return true;
                }
            }
            Stmt::For(for_stmt) => {
                // for(;;) — init, test, update all None
                if for_stmt.init.is_some() || for_stmt.test.is_some() || for_stmt.update.is_some() {
                    continue;
                }
                if loop_body_has_state_switch(for_stmt.body.as_ref(), param_name) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn is_true_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(Lit::Bool(b)) => b.value,
        Expr::Lit(Lit::Num(n)) => n.value != 0.0,
        _ => false,
    }
}

/// Check if a block contains `switch(param.prev = param.next) { ... }` or
/// Babel 7.27+ `switch(param.n) { ... }`.
fn has_state_switch(block: &BlockStmt, param_name: &Atom) -> bool {
    for stmt in &block.stmts {
        if let Stmt::Switch(sw) = stmt {
            if is_state_switch_discriminant(&sw.discriminant, param_name) {
                return true;
            }
        }
    }
    false
}

fn loop_body_has_state_switch(stmt: &Stmt, param_name: &Atom) -> bool {
    match stmt {
        Stmt::Switch(sw) => is_state_switch_discriminant(&sw.discriminant, param_name),
        Stmt::Block(block) => has_state_switch(block, param_name),
        _ => false,
    }
}

fn switch_cases_from_loop_body_ref(stmt: &Stmt) -> Option<&[SwitchCase]> {
    match stmt {
        Stmt::Switch(sw) => Some(&sw.cases),
        Stmt::Block(block) => {
            for inner in &block.stmts {
                if let Stmt::Switch(sw) = inner {
                    return Some(&sw.cases);
                }
            }
            None
        }
        _ => None,
    }
}

fn is_state_switch_discriminant(expr: &Expr, param_name: &Atom) -> bool {
    if let Expr::Member(member) = expr {
        return is_ident_with_name(&member.obj, param_name) && is_next_prop(&member.prop);
    }

    let Expr::Assign(assign) = expr else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }
    // Left: param.prev or Babel 7.27+ param.p
    let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) else {
        return false;
    };
    if !is_ident_with_name(&left_member.obj, param_name) || !is_prev_prop(&left_member.prop) {
        return false;
    }
    let Expr::Member(right_member) = assign.right.as_ref() else {
        return false;
    };
    is_ident_with_name(&right_member.obj, param_name) && is_next_prop(&right_member.prop)
}

fn extract_wrap_args(stmt: Stmt) -> Option<(Atom, Vec<SwitchCase>, Vec<[Option<usize>; 4]>)> {
    let Stmt::Return(ret) = stmt else { return None };
    let arg = *ret.arg?;
    let Expr::Call(call) = arg else { return None };
    let callee_expr = call.callee.as_expr()?;
    let Expr::Member(member) = callee_expr.as_ref() else {
        return None;
    };
    if !is_wrap_prop(&member.prop) {
        return None;
    }
    if call.args.is_empty() {
        return None;
    }

    let try_regions = extract_wrap_try_regions_from_call(&call);
    let fn_arg = *call.args.into_iter().next()?.expr;
    let (state_name, cases) = extract_state_machine_parts(fn_arg)?;
    Some((state_name, cases, try_regions))
}

fn extract_wrap_try_regions_from_call(
    call: &swc_core::ecma::ast::CallExpr,
) -> Vec<[Option<usize>; 4]> {
    let Some(arg4) = call.args.get(3) else {
        return Vec::new();
    };
    let Expr::Array(arr) = arg4.expr.as_ref() else {
        return Vec::new();
    };
    arr.elems
        .iter()
        .filter_map(|elem| {
            let elem = elem.as_ref()?;
            let Expr::Array(region) = elem.expr.as_ref() else {
                return None;
            };
            parse_try_region_array(region)
        })
        .collect()
}

fn parse_try_region_array(arr: &ArrayLit) -> Option<[Option<usize>; 4]> {
    if arr.elems.len() < 2 {
        return None;
    }
    let mut region = [None; 4];
    for (i, elem) in arr.elems.iter().enumerate().take(4) {
        region[i] = elem.as_ref().and_then(|e| {
            if let Expr::Lit(Lit::Num(n)) = e.expr.as_ref() {
                Some(n.value as usize)
            } else {
                None
            }
        });
    }
    Some(region)
}

fn extract_state_machine_parts(expr: Expr) -> Option<(Atom, Vec<SwitchCase>)> {
    match expr {
        Expr::Fn(fn_expr) => {
            let param_name = match &fn_expr.function.params.first()?.pat {
                Pat::Ident(bi) => bi.id.sym.clone(),
                _ => return None,
            };
            let body = fn_expr.function.body?;
            let cases = extract_switch_cases_from_body(body)?;
            Some((param_name, cases))
        }
        Expr::Arrow(arrow) => {
            let param_name = match &arrow.params.first()? {
                Pat::Ident(bi) => bi.id.sym.clone(),
                _ => return None,
            };
            let body = match *arrow.body {
                swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(b) => b,
                _ => return None,
            };
            let cases = extract_switch_cases_from_body(body)?;
            Some((param_name, cases))
        }
        _ => None,
    }
}

fn extract_switch_cases_from_body(body: BlockStmt) -> Option<Vec<SwitchCase>> {
    // Find the while(true) or for(;;) loop, then the switch inside
    for stmt in body.stmts {
        match stmt {
            Stmt::While(while_stmt) => {
                if let Some(cases) = switch_cases_from_loop_body(*while_stmt.body) {
                    return Some(cases);
                }
            }
            Stmt::For(for_stmt) => {
                if let Some(cases) = switch_cases_from_loop_body(*for_stmt.body) {
                    return Some(cases);
                }
            }
            _ => {}
        }
    }
    None
}

fn switch_cases_from_loop_body(stmt: Stmt) -> Option<Vec<SwitchCase>> {
    match stmt {
        Stmt::Switch(sw) => Some(sw.cases),
        Stmt::Block(block) => {
            for inner in block.stmts {
                if let Stmt::Switch(sw) = inner {
                    return Some(sw.cases);
                }
            }
            None
        }
        _ => None,
    }
}

// ============================================================
// Babel state machine decoder
// ============================================================

fn decode_babel_state_machine(
    state_name: &Atom,
    cases: Vec<SwitchCase>,
    mut trys: Vec<[Option<usize>; 4]>,
) -> Vec<Stmt> {
    infer_try_region_nexts(&mut trys, &cases);
    // Collect (label_idx, stmt) pairs
    let mut flat: Vec<(usize, Stmt)> = Vec::new();
    let mut skip_delegate_result_assignments: HashSet<(usize, usize)> = HashSet::new();

    for case in &cases {
        let idx = match case_label_index(case) {
            Some(n) => n,
            None => continue, // skip "end" case
        };

        let mut catch_aliases = Vec::new();
        let is_catch = is_catch_label(idx, &trys);
        let stmts = &case.cons;
        let mut i = 0;
        while i < stmts.len() {
            let stmt = &stmts[i];

            if skip_delegate_result_assignments.contains(&(idx, i)) {
                i += 1;
                continue;
            }

            // Detect _ctx.next = N; break; pairs. When N < idx (a back-edge),
            // preserve the goto as `return [3, N]` so `recover_index_loops`
            // can reconstruct for-loops. Forward/fallthrough gotos are dropped.
            if is_next_assign(state_name, stmt) {
                if let Some(target) = extract_state_next_assign_target(state_name, stmt) {
                    if i + 1 < stmts.len() && matches!(stmts[i + 1], Stmt::Break(_)) {
                        if target > 0 && target < idx {
                            flat.push((idx, jump_return_stmt(target)));
                        }
                        i += 2;
                        continue;
                    }
                }
                i += 1;
                continue;
            }

            // Skip Babel's `_ctx.prev = N` / `_ctx.p = N` bookkeeping.
            if is_prev_assign(state_name, stmt) {
                i += 1;
                continue;
            }

            // Skip _ctx.label = N (tslib-style, shouldn't appear but be safe)
            if is_label_assign(state_name, stmt) {
                i += 1;
                continue;
            }

            // Handle _ctx.trys.push([...]) for try/catch regions
            if let Some(region) = extract_trys_push(state_name, stmt) {
                trys.push(region);
                i += 1;
                continue;
            }

            if is_catch {
                if let Some(alias) = extract_catch_value_alias(state_name, stmt) {
                    catch_aliases.push(alias);
                    i += 1;
                    continue;
                }
            }

            let mut stmt = stmt.clone();
            if is_catch {
                let mut replacer = CatchValueReplacer {
                    state_name: state_name.clone(),
                    aliases: catch_aliases.clone(),
                    replacement: Box::new(Expr::Ident(Ident::new_no_ctxt(
                        "error".into(),
                        DUMMY_SP,
                    ))),
                };
                stmt.visit_mut_with(&mut replacer);
            }

            if let Some((decoded, consumed)) = decode_nested_state_jump(state_name, &stmts[i..]) {
                flat.push((idx, decoded));
                i += consumed;
                continue;
            }

            // Handle return statements (yields, abrupt returns, stop)
            if let Stmt::Return(ret) = &stmt {
                if let Some(decoded) = decode_return(state_name, ret) {
                    match decoded {
                        DecodedReturn::Return(expr) => {
                            flat.push((
                                idx,
                                Stmt::Return(ReturnStmt {
                                    span: DUMMY_SP,
                                    arg: Some(expr),
                                }),
                            ));
                        }
                        DecodedReturn::ReturnVoid => {
                            flat.push((
                                idx,
                                Stmt::Return(ReturnStmt {
                                    span: DUMMY_SP,
                                    arg: None,
                                }),
                            ));
                        }
                        DecodedReturn::Throw(expr) => {
                            flat.push((
                                idx,
                                Stmt::Throw(swc_core::ecma::ast::ThrowStmt {
                                    span: DUMMY_SP,
                                    arg: expr,
                                }),
                            ));
                        }
                        DecodedReturn::Stop => {} // end of generator, drop
                        DecodedReturn::CommaYield(expr) => {
                            // return _ctx.next = N, value → yield value
                            flat.push((
                                idx,
                                Stmt::Expr(ExprStmt {
                                    span: DUMMY_SP,
                                    expr: Box::new(Expr::Yield(YieldExpr {
                                        span: DUMMY_SP,
                                        delegate: false,
                                        arg: Some(expr),
                                    })),
                                }),
                            ));
                        }
                        DecodedReturn::DelegateYield {
                            expr,
                            result_name,
                            next_loc,
                        } => {
                            if let (Some(result_name), Some(next_loc)) =
                                (result_name.as_ref(), next_loc)
                            {
                                if let Some((assign_index, assign_stmt)) =
                                    extract_delegate_result_assignment(
                                        state_name,
                                        &cases,
                                        next_loc,
                                        result_name,
                                        expr.clone(),
                                    )
                                {
                                    skip_delegate_result_assignments
                                        .insert((next_loc, assign_index));
                                    flat.push((idx, assign_stmt));
                                } else {
                                    flat.push((
                                        idx,
                                        Stmt::Expr(ExprStmt {
                                            span: DUMMY_SP,
                                            expr: delegate_yield_expr(expr),
                                        }),
                                    ));
                                }
                            } else {
                                flat.push((
                                    idx,
                                    Stmt::Expr(ExprStmt {
                                        span: DUMMY_SP,
                                        expr: delegate_yield_expr(expr),
                                    }),
                                ));
                            }
                        }
                    }
                    i += 1;
                    continue;
                }
                // Plain return with non-pattern expression: treat as yield
                if let Some(arg) = &ret.arg {
                    if !is_stop_call(state_name, arg) {
                        flat.push((
                            idx,
                            Stmt::Expr(ExprStmt {
                                span: DUMMY_SP,
                                expr: Box::new(Expr::Yield(YieldExpr {
                                    span: DUMMY_SP,
                                    delegate: false,
                                    arg: Some(arg.clone()),
                                })),
                            }),
                        ));
                        i += 1;
                        continue;
                    }
                }
                i += 1;
                continue;
            }

            // Handle break — if the last _ctx.next pointed back to case 0 this is a loop,
            // otherwise it's just a goto (skip it)
            if matches!(stmt, Stmt::Break(_)) {
                i += 1;
                continue;
            }

            // Regular statement — emit as-is
            flat.push((idx, stmt));
            i += 1;
        }
    }

    // Phase 2: merge _ctx.sent with previous yield
    let mut output: Vec<(usize, Stmt)> = Vec::new();
    for (idx, stmt) in flat {
        if is_standalone_sent(state_name, &stmt) {
            continue;
        }
        if stmt_uses_sent(state_name, &stmt) {
            if is_catch_label(idx, &trys) {
                let mut replacer = SentReplacer {
                    state_name: state_name.clone(),
                    replacement: Box::new(Expr::Ident(Ident::new_no_ctxt(
                        "error".into(),
                        DUMMY_SP,
                    ))),
                };
                let mut s = stmt;
                s.visit_mut_with(&mut replacer);
                output.push((idx, s));
                continue;
            }
            let merged = if let Some((_, prev)) = output.last() {
                extract_yield_from_stmt(prev).map(|arg| {
                    let yield_expr = Box::new(Expr::Yield(YieldExpr {
                        span: DUMMY_SP,
                        delegate: false,
                        arg: Some(arg),
                    }));
                    let mut replacer = SentReplacer {
                        state_name: state_name.clone(),
                        replacement: yield_expr,
                    };
                    let mut s = stmt.clone();
                    s.visit_mut_with(&mut replacer);
                    s
                })
            } else {
                None
            };
            if let Some(merged_stmt) = merged {
                output.pop();
                output.push((idx, merged_stmt));
            } else {
                let mut replacer = SentReplacer {
                    state_name: state_name.clone(),
                    replacement: Box::new(Expr::Ident(Ident::new_no_ctxt(
                        "undefined".into(),
                        DUMMY_SP,
                    ))),
                };
                let mut s = stmt;
                s.visit_mut_with(&mut replacer);
                output.push((idx, s));
            }
        } else {
            output.push((idx, stmt));
        }
    }

    // Phase 3: Detect infinite loops (case 0 → ... → goto 0 pattern)
    let has_back_edge_to_zero = detect_back_edge_to_zero(state_name, &cases);

    let mut result = recover_index_loops(
        StateMachineProgram::from_labeled_stmts(output, trys)
            .recover_conditional_assignments()
            .resolve_labeled_forward_jumps(OpcodeReturnScan::IncludeNestedFunctions)
            .into_reconstructed_stmts(),
    );
    fold_state_temp_member_calls(state_name, &mut result);

    // Wrap in while(true) if we detected a back-edge to case 0
    if has_back_edge_to_zero && !result.is_empty() {
        result = vec![Stmt::While(WhileStmt {
            span: DUMMY_SP,
            test: Box::new(Expr::Lit(Lit::Bool(swc_core::ecma::ast::Bool {
                span: DUMMY_SP,
                value: true,
            }))),
            body: Box::new(Stmt::Block(BlockStmt {
                span: DUMMY_SP,
                ctxt: Default::default(),
                stmts: result,
            })),
        })];
    }

    result
}

fn detect_back_edge_to_zero(state_name: &Atom, cases: &[SwitchCase]) -> bool {
    for case in cases {
        let idx = match case_label_index(case) {
            Some(n) => n,
            None => continue,
        };
        if idx == 0 {
            continue;
        }
        for stmt in &case.cons {
            if is_next_assign_to(state_name, stmt, 0) {
                return true;
            }
            // Check comma operator: return _ctx.next = 0, ...
            if let Stmt::Return(ret) = stmt {
                if let Some(arg) = &ret.arg {
                    if is_comma_next_assign_to(state_name, arg, 0) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn decode_nested_state_jump(state_name: &Atom, stmts: &[Stmt]) -> Option<(Stmt, usize)> {
    let Stmt::If(if_stmt) = stmts.first()? else {
        return None;
    };
    if if_stmt.alt.is_some() {
        return None;
    }
    let goto_target = extract_next_break_target(state_name, &if_stmt.cons)?;

    if let Some(continue_target) = stmts
        .get(1)
        .and_then(|stmt| extract_continue_target(state_name, stmt))
    {
        if continue_target != goto_target {
            return Some((
                jump_if_stmt(invert_condition(&if_stmt.test), continue_target),
                2,
            ));
        }
    }

    Some((jump_if_stmt(if_stmt.test.clone(), goto_target), 1))
}

fn extract_next_break_target(state_name: &Atom, stmt: &Stmt) -> Option<usize> {
    let Stmt::Block(block) = stmt else {
        return None;
    };
    if block.stmts.len() != 2 || !matches!(block.stmts[1], Stmt::Break(_)) {
        return None;
    }
    extract_state_next_assign_target(state_name, &block.stmts[0])
}

fn extract_state_next_assign_target(state_name: &Atom, stmt: &Stmt) -> Option<usize> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let left_member = assign.left.as_simple().and_then(|s| s.as_member())?;
    if !is_ident_with_name(&left_member.obj, state_name) || !is_next_prop(&left_member.prop) {
        return None;
    }
    number_lit_usize(&assign.right)
}

fn extract_continue_target(state_name: &Atom, stmt: &Stmt) -> Option<usize> {
    extract_abrupt_continue_target(state_name, stmt)
        .or_else(|| extract_short_continue_target(state_name, stmt))
}

fn extract_abrupt_continue_target(state_name: &Atom, stmt: &Stmt) -> Option<usize> {
    let Stmt::Return(ret) = stmt else {
        return None;
    };
    let Expr::Call(call) = ret.arg.as_deref()? else {
        return None;
    };
    let callee = call.callee.as_expr()?;
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    if !is_ident_with_name(&member.obj, state_name) || !member_prop_name(&member.prop, "abrupt") {
        return None;
    }
    let Expr::Lit(Lit::Str(kind)) = call.args.first()?.expr.as_ref() else {
        return None;
    };
    if kind.value.as_str() != Some("continue") {
        return None;
    }
    number_lit_usize(&call.args.get(1)?.expr)
}

fn extract_short_continue_target(state_name: &Atom, stmt: &Stmt) -> Option<usize> {
    let Stmt::Return(ret) = stmt else {
        return None;
    };
    let Expr::Call(call) = ret.arg.as_deref()? else {
        return None;
    };
    let callee = call.callee.as_expr()?;
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    if !is_ident_with_name(&member.obj, state_name) || !member_prop_name(&member.prop, "a") {
        return None;
    }
    let Expr::Lit(Lit::Num(kind)) = call.args.first()?.expr.as_ref() else {
        return None;
    };
    if kind.value as u8 != 3 {
        return None;
    }
    number_lit_usize(&call.args.get(1)?.expr)
}

fn jump_if_stmt(test: Box<Expr>, target: usize) -> Stmt {
    Stmt::If(swc_core::ecma::ast::IfStmt {
        span: DUMMY_SP,
        test,
        cons: Box::new(jump_return_stmt(target)),
        alt: None,
    })
}

fn jump_return_stmt(target: usize) -> Stmt {
    Stmt::Return(ReturnStmt {
        span: DUMMY_SP,
        arg: Some(Box::new(Expr::Array(ArrayLit {
            span: DUMMY_SP,
            elems: vec![
                Some(number_array_elem(3.0)),
                Some(number_array_elem(target as f64)),
            ],
        }))),
    })
}

fn number_array_elem(value: f64) -> ExprOrSpread {
    ExprOrSpread {
        spread: None,
        expr: Box::new(Expr::Lit(Lit::Num(Number {
            span: DUMMY_SP,
            value,
            raw: None,
        }))),
    }
}

fn infer_try_region_nexts(trys: &mut [[Option<usize>; 4]], cases: &[SwitchCase]) {
    for region in trys.iter_mut() {
        if region[3].is_some() {
            continue;
        }
        let Some(start) = region[0] else {
            continue;
        };
        let Some(region_body_end) = region[1].or(region[2]) else {
            continue;
        };
        let next = cases
            .iter()
            .filter_map(|case| {
                let idx = case_label_index(case)?;
                if idx < start || idx >= region_body_end {
                    return None;
                }
                case.cons
                    .iter()
                    .filter_map(extract_next_assign_target)
                    .filter(|target| *target > region_body_end)
                    .min()
            })
            .min();
        region[3] = next;
    }
}

fn is_next_assign_to(state_name: &Atom, stmt: &Stmt, target: usize) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }
    let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) else {
        return false;
    };
    if !is_ident_with_name(&left_member.obj, state_name) || !is_next_prop(&left_member.prop) {
        return false;
    }
    if let Expr::Lit(Lit::Num(n)) = assign.right.as_ref() {
        return n.value as usize == target;
    }
    false
}

fn is_comma_next_assign_to(state_name: &Atom, expr: &Expr, target: usize) -> bool {
    let Expr::Seq(seq) = expr else {
        return false;
    };
    for e in &seq.exprs {
        if let Expr::Assign(assign) = e.as_ref() {
            if assign.op != AssignOp::Assign {
                continue;
            }
            let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) else {
                continue;
            };
            if is_ident_with_name(&left_member.obj, state_name) && is_next_prop(&left_member.prop) {
                if let Expr::Lit(Lit::Num(n)) = assign.right.as_ref() {
                    if n.value as usize == target {
                        return true;
                    }
                }
            }
        }
    }
    false
}

enum DecodedReturn {
    Return(Box<Expr>),
    ReturnVoid,
    Throw(Box<Expr>),
    Stop,
    CommaYield(Box<Expr>),
    DelegateYield {
        expr: Box<Expr>,
        result_name: Option<Atom>,
        next_loc: Option<usize>,
    },
}

fn decode_return(state_name: &Atom, ret: &ReturnStmt) -> Option<DecodedReturn> {
    let arg = ret.arg.as_ref()?;

    // return _ctx.stop()
    if is_stop_call(state_name, arg) || is_finish_call(state_name, arg) {
        return Some(DecodedReturn::Stop);
    }

    if let Some(decoded) = decode_delegate_yield(state_name, arg) {
        return Some(decoded);
    }

    // return _ctx.abrupt("return", value)
    if let Some(decoded) = decode_abrupt(state_name, arg) {
        return Some(decoded);
    }

    // Babel 7.27+: return _ctx.a(2, value)
    if let Some(decoded) = decode_short_abrupt(state_name, arg) {
        return Some(decoded);
    }

    // return (_ctx.next = N, value) — comma operator form
    if let Expr::Seq(seq) = arg.as_ref() {
        if seq.exprs.len() >= 2 {
            // Check if first expression is _ctx.next = N
            if is_next_assign_expr(state_name, &seq.exprs[0]) {
                let value = seq.exprs.last().unwrap().clone();
                return Some(DecodedReturn::CommaYield(value));
            }
        }
    }

    None
}

fn decode_delegate_yield(state_name: &Atom, expr: &Expr) -> Option<DecodedReturn> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let callee = call.callee.as_expr()?;
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    if !is_ident_with_name(&member.obj, state_name)
        || !(member_prop_name(&member.prop, "delegateYield") || member_prop_name(&member.prop, "d"))
    {
        return None;
    }
    let arg = call.args.first()?.expr.clone();
    let result_name = call.args.get(1).and_then(|arg| {
        if let Expr::Lit(Lit::Str(str_lit)) = arg.expr.as_ref() {
            str_lit.value.as_str().map(Atom::from)
        } else {
            None
        }
    });
    let next_arg = if result_name.is_some() {
        call.args.get(2)
    } else {
        call.args.get(1)
    };
    let next_loc = next_arg.and_then(|arg| number_lit_usize(&arg.expr));
    Some(DecodedReturn::DelegateYield {
        // Name-based unwrap during decode; the post-pass handles minified
        // aliases via detected helper bindings.
        expr: unwrap_regenerator_values(arg, &[]),
        result_name,
        next_loc,
    })
}

fn extract_delegate_result_assignment(
    state_name: &Atom,
    cases: &[SwitchCase],
    next_loc: usize,
    result_name: &Atom,
    yielded: Box<Expr>,
) -> Option<(usize, Stmt)> {
    let case = cases
        .iter()
        .find(|case| case_label_index(case) == Some(next_loc))?;
    for (stmt_index, stmt) in case.cons.iter().enumerate() {
        if is_next_assign(state_name, stmt)
            || is_prev_assign(state_name, stmt)
            || is_label_assign(state_name, stmt)
        {
            continue;
        }
        return rewrite_delegate_result_assignment(state_name, result_name, yielded, stmt)
            .map(|stmt| (stmt_index, stmt));
    }
    None
}

fn rewrite_delegate_result_assignment(
    state_name: &Atom,
    result_name: &Atom,
    yielded: Box<Expr>,
    stmt: &Stmt,
) -> Option<Stmt> {
    match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => {
            let Expr::Assign(assign) = expr.as_ref() else {
                return None;
            };
            if assign.op != AssignOp::Assign
                || !is_state_result_expr(state_name, &assign.right, result_name)
            {
                return None;
            }
            let mut assign = assign.clone();
            assign.right = delegate_yield_expr(yielded);
            Some(Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(Expr::Assign(assign)),
            }))
        }
        Stmt::Decl(Decl::Var(var_decl)) => {
            let mut changed = false;
            let mut var_decl = var_decl.clone();
            for decl in &mut var_decl.decls {
                if decl
                    .init
                    .as_deref()
                    .is_some_and(|init| is_state_result_expr(state_name, init, result_name))
                {
                    decl.init = Some(delegate_yield_expr(yielded.clone()));
                    changed = true;
                }
            }
            changed.then_some(Stmt::Decl(Decl::Var(var_decl)))
        }
        _ => None,
    }
}

fn delegate_yield_expr(expr: Box<Expr>) -> Box<Expr> {
    Box::new(Expr::Yield(YieldExpr {
        span: DUMMY_SP,
        delegate: true,
        arg: Some(expr),
    }))
}

fn is_state_result_expr(state_name: &Atom, expr: &Expr, result_name: &Atom) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    is_ident_with_name(&member.obj, state_name)
        && member_prop_atom(&member.prop).as_ref() == Some(result_name)
}

fn unwrap_regenerator_values(expr: Box<Expr>, helpers: &[BindingKey]) -> Box<Expr> {
    let Expr::Call(call) = expr.as_ref() else {
        return expr;
    };
    let Some(callee) = call.callee.as_expr() else {
        return expr;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return expr;
    };
    // Match the canonical `_regeneratorValues` name, or a detected helper
    // binding (robust to minified/top-level-mangled aliases).
    let is_values_helper = id.sym.as_ref().contains("regeneratorValues")
        || helpers
            .iter()
            .any(|(sym, ctxt)| id.sym == *sym && id.ctxt == *ctxt);
    if is_values_helper {
        return call
            .args
            .first()
            .map(|arg| arg.expr.clone())
            .unwrap_or(expr);
    }
    expr
}

fn is_stop_call(state_name: &Atom, expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Some(callee) = call.callee.as_expr() else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    is_ident_with_name(&member.obj, state_name) && member_prop_name(&member.prop, "stop")
}

fn is_finish_call(state_name: &Atom, expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Some(callee) = call.callee.as_expr() else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    is_ident_with_name(&member.obj, state_name)
        && (member_prop_name(&member.prop, "finish") || member_prop_name(&member.prop, "f"))
}

fn decode_abrupt(state_name: &Atom, expr: &Expr) -> Option<DecodedReturn> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let callee = call.callee.as_expr()?;
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    if !is_ident_with_name(&member.obj, state_name) || !member_prop_name(&member.prop, "abrupt") {
        return None;
    }
    if call.args.is_empty() {
        return None;
    }
    let Expr::Lit(Lit::Str(kind)) = call.args[0].expr.as_ref() else {
        return None;
    };
    let kind_str = kind.value.as_str().unwrap_or("");
    match kind_str {
        "return" => {
            if call.args.len() >= 2 {
                Some(DecodedReturn::Return(call.args[1].expr.clone()))
            } else {
                Some(DecodedReturn::ReturnVoid)
            }
        }
        "throw" => {
            if call.args.len() >= 2 {
                Some(DecodedReturn::Throw(call.args[1].expr.clone()))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn decode_short_abrupt(state_name: &Atom, expr: &Expr) -> Option<DecodedReturn> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let callee = call.callee.as_expr()?;
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    if !is_ident_with_name(&member.obj, state_name) || !member_prop_name(&member.prop, "a") {
        return None;
    }
    let Expr::Lit(Lit::Num(kind)) = call.args.first()?.expr.as_ref() else {
        return None;
    };
    match kind.value as u8 {
        1 => call
            .args
            .get(1)
            .map(|arg| DecodedReturn::Throw(arg.expr.clone())),
        2 => call
            .args
            .get(1)
            .map(|arg| DecodedReturn::Return(arg.expr.clone()))
            .or(Some(DecodedReturn::Stop)),
        _ => None,
    }
}

fn is_next_assign(state_name: &Atom, stmt: &Stmt) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    is_next_assign_expr(state_name, expr)
}

fn extract_next_assign_target(stmt: &Stmt) -> Option<usize> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let left_member = assign.left.as_simple().and_then(|s| s.as_member())?;
    if !is_next_prop(&left_member.prop) {
        return None;
    }
    let Expr::Lit(Lit::Num(n)) = assign.right.as_ref() else {
        return None;
    };
    Some(n.value as usize)
}

fn is_next_assign_expr(state_name: &Atom, expr: &Expr) -> bool {
    let Expr::Assign(assign) = expr else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }
    let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) else {
        return false;
    };
    is_ident_with_name(&left_member.obj, state_name) && is_next_prop(&left_member.prop)
}

fn is_next_prop(prop: &MemberProp) -> bool {
    member_prop_name(prop, "next") || member_prop_name(prop, "n")
}

fn is_wrap_prop(prop: &MemberProp) -> bool {
    member_prop_name(prop, "wrap") || member_prop_name(prop, "w")
}

fn is_mark_prop(prop: &MemberProp) -> bool {
    member_prop_name(prop, "mark") || member_prop_name(prop, "m")
}

fn is_label_assign(state_name: &Atom, stmt: &Stmt) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }
    let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) else {
        return false;
    };
    is_ident_with_name(&left_member.obj, state_name) && member_prop_name(&left_member.prop, "label")
}

fn is_prev_assign(state_name: &Atom, stmt: &Stmt) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }
    let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) else {
        return false;
    };
    is_ident_with_name(&left_member.obj, state_name) && is_prev_prop(&left_member.prop)
}

fn is_prev_prop(prop: &MemberProp) -> bool {
    member_prop_name(prop, "prev") || member_prop_name(prop, "p")
}

fn case_label_index(case: &SwitchCase) -> Option<usize> {
    let test = case.test.as_ref()?;
    if let Expr::Lit(Lit::Num(n)) = test.as_ref() {
        Some(n.value as usize)
    } else {
        None // "end" case
    }
}

fn extract_trys_push(state_name: &Atom, stmt: &Stmt) -> Option<[Option<usize>; 4]> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let callee_expr = call.callee.as_expr()?;
    let Expr::Member(callee_mem) = callee_expr.as_ref() else {
        return None;
    };
    let Expr::Member(outer_mem) = callee_mem.obj.as_ref() else {
        return None;
    };
    if !is_ident_with_name(&outer_mem.obj, state_name) {
        return None;
    }
    if !member_prop_name(&outer_mem.prop, "trys") {
        return None;
    }
    if !member_prop_name(&callee_mem.prop, "push") {
        return None;
    }
    if call.args.len() != 1 {
        return None;
    }
    let Expr::Array(arr) = call.args[0].expr.as_ref() else {
        return None;
    };
    parse_try_region_array(arr)
}

fn is_standalone_sent(state_name: &Atom, stmt: &Stmt) -> bool {
    if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
        return is_sent_access(state_name, expr);
    }
    false
}

fn is_sent_access(state_name: &Atom, expr: &Expr) -> bool {
    // _ctx.sent (property access, not method call like tslib)
    if let Expr::Member(member) = expr {
        return is_ident_with_name(&member.obj, state_name) && is_sent_prop(&member.prop);
    }
    // Also handle _ctx.sent() (some versions use method call)
    if let Expr::Call(call) = expr {
        if let Some(callee) = call.callee.as_expr() {
            if let Expr::Member(member) = callee.as_ref() {
                return is_ident_with_name(&member.obj, state_name) && is_sent_prop(&member.prop);
            }
        }
    }
    false
}

fn stmt_uses_sent(state_name: &Atom, stmt: &Stmt) -> bool {
    struct Finder {
        state_name: Atom,
        found: bool,
    }
    impl swc_core::ecma::visit::Visit for Finder {
        fn visit_function(&mut self, _func: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}

        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if is_ident_with_name(&member.obj, &self.state_name) && is_sent_prop(&member.prop) {
                self.found = true;
                return;
            }
            member.visit_children_with(self);
        }
    }
    let mut f = Finder {
        state_name: state_name.clone(),
        found: false,
    };
    stmt.visit_with(&mut f);
    f.found
}

fn extract_yield_from_stmt(stmt: &Stmt) -> Option<Box<Expr>> {
    if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
        if let Expr::Yield(y) = expr.as_ref() {
            return y.arg.clone();
        }
    }
    None
}

fn is_catch_label(label_idx: usize, trys: &[[Option<usize>; 4]]) -> bool {
    trys.iter().any(|region| region[1] == Some(label_idx))
}

struct SentReplacer {
    state_name: Atom,
    replacement: Box<Expr>,
}

#[derive(Clone)]
enum CatchValueAlias {
    StateMember(Atom),
    LocalIdent(BindingKey),
}

fn extract_catch_value_alias(state_name: &Atom, stmt: &Stmt) -> Option<CatchValueAlias> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }

    if is_state_catch_call(state_name, &assign.right) {
        let left_member = assign.left.as_simple()?.as_member()?;
        if !is_ident_with_name(&left_member.obj, state_name) {
            return None;
        }
        return member_prop_atom(&left_member.prop).map(CatchValueAlias::StateMember);
    }

    if is_sent_access(state_name, &assign.right) {
        let ident = assign.left.as_simple()?.as_ident()?;
        return Some(CatchValueAlias::LocalIdent((ident.sym.clone(), ident.ctxt)));
    }

    None
}

fn is_state_catch_call(state_name: &Atom, expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Some(callee) = call.callee.as_expr() else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    is_ident_with_name(&member.obj, state_name) && member_prop_name(&member.prop, "catch")
}

struct CatchValueReplacer {
    state_name: Atom,
    aliases: Vec<CatchValueAlias>,
    replacement: Box<Expr>,
}

impl VisitMut for CatchValueReplacer {
    fn visit_mut_function(&mut self, _func: &mut Function) {}

    fn visit_mut_arrow_expr(&mut self, _arrow: &mut ArrowExpr) {}

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if is_sent_access(&self.state_name, expr) {
            *expr = *self.replacement.clone();
            return;
        }

        if let Expr::Ident(id) = expr {
            if self.aliases.iter().any(|alias| {
                matches!(alias, CatchValueAlias::LocalIdent((sym, ctxt)) if id.sym == *sym && id.ctxt == *ctxt)
            }) {
                *expr = *self.replacement.clone();
                return;
            }
        }

        if let Expr::Member(member) = expr {
            if is_ident_with_name(&member.obj, &self.state_name) {
                if let Some(prop) = member_prop_atom(&member.prop) {
                    if self.aliases.iter().any(|alias| {
                        matches!(alias, CatchValueAlias::StateMember(alias_prop) if *alias_prop == prop)
                    }) {
                        *expr = *self.replacement.clone();
                        return;
                    }
                }
            }
        }

        expr.visit_mut_children_with(self);
    }
}

impl VisitMut for SentReplacer {
    fn visit_mut_function(&mut self, _func: &mut Function) {}

    fn visit_mut_arrow_expr(&mut self, _arrow: &mut ArrowExpr) {}

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        // Replace _ctx.sent property access
        if let Expr::Member(member) = expr {
            if is_ident_with_name(&member.obj, &self.state_name) && is_sent_prop(&member.prop) {
                *expr = *self.replacement.clone();
                return;
            }
        }
        // Replace _ctx.sent() method call
        if let Expr::Call(call) = expr {
            if let Some(callee) = call.callee.as_expr() {
                if let Expr::Member(member) = callee.as_ref() {
                    if is_ident_with_name(&member.obj, &self.state_name)
                        && is_sent_prop(&member.prop)
                    {
                        *expr = *self.replacement.clone();
                        return;
                    }
                }
            }
        }
        expr.visit_mut_children_with(self);
    }
}

fn is_sent_prop(prop: &MemberProp) -> bool {
    member_prop_name(prop, "sent") || member_prop_name(prop, "v")
}

fn recover_index_loops(stmts: Vec<Stmt>) -> Vec<Stmt> {
    let mut result = Vec::new();
    let mut index = 0usize;

    while index < stmts.len() {
        if let Some((loop_stmt, consumed)) = try_recover_index_loop(&stmts[index..]) {
            result.push(loop_stmt);
            index += consumed;
        } else {
            result.push(stmts[index].clone());
            index += 1;
        }
    }

    result
}

fn try_recover_index_loop(stmts: &[Stmt]) -> Option<(Stmt, usize)> {
    let (test, break_target) = loop_break_test(stmts.first()?)?;
    let final_return_idx = find_loop_boundary(stmts)?;
    if final_return_idx < 3 {
        return None;
    }

    let update_idx = final_return_idx.checked_sub(1)?;
    let update = expr_stmt_expr(&stmts[update_idx])?;
    let mut body_stmts = stmts[1..update_idx].to_vec();
    let continue_target = single_continue_target(&body_stmts, break_target).or_else(|| {
        return_jump_target(&stmts[final_return_idx]).filter(|target| *target < break_target)
    })?;
    convert_jump_returns(&mut body_stmts, break_target, continue_target)?;

    let consumed = if return_jump_target(&stmts[final_return_idx]).is_some() {
        final_return_idx + 1
    } else {
        update_idx + 1
    };
    Some((
        Stmt::For(ForStmt {
            span: DUMMY_SP,
            init: None,
            test: Some(test),
            update: Some(update),
            body: Box::new(Stmt::Block(BlockStmt {
                span: DUMMY_SP,
                ctxt: Default::default(),
                stmts: body_stmts,
            })),
        }),
        consumed,
    ))
}

fn single_continue_target(stmts: &[Stmt], break_target: usize) -> Option<usize> {
    let mut targets = HashSet::new();
    collect_jump_targets(stmts, &mut targets);
    targets.remove(&break_target);
    if targets.len() == 1 {
        targets.into_iter().next()
    } else {
        None
    }
}

fn collect_jump_targets(stmts: &[Stmt], targets: &mut HashSet<usize>) {
    for stmt in stmts {
        match stmt {
            Stmt::Return(_) => {
                if let Some(target) = return_jump_target(stmt) {
                    targets.insert(target);
                }
            }
            Stmt::If(if_stmt) => {
                collect_jump_target(&if_stmt.cons, targets);
                if let Some(alt) = &if_stmt.alt {
                    collect_jump_target(alt, targets);
                }
            }
            Stmt::Block(block) => collect_jump_targets(&block.stmts, targets),
            Stmt::Try(try_stmt) => {
                collect_jump_targets(&try_stmt.block.stmts, targets);
                if let Some(handler) = &try_stmt.handler {
                    collect_jump_targets(&handler.body.stmts, targets);
                }
                if let Some(finalizer) = &try_stmt.finalizer {
                    collect_jump_targets(&finalizer.stmts, targets);
                }
            }
            _ => {}
        }
    }
}

fn collect_jump_target(stmt: &Stmt, targets: &mut HashSet<usize>) {
    collect_jump_targets(std::slice::from_ref(stmt), targets);
}

fn loop_break_test(stmt: &Stmt) -> Option<(Box<Expr>, usize)> {
    let Stmt::If(if_stmt) = stmt else {
        return None;
    };
    if if_stmt.alt.is_some() {
        return None;
    }
    let target = jump_target_stmt(&if_stmt.cons)?;
    Some((invert_condition(&if_stmt.test), target))
}

/// Find the loop boundary return. A top-level back-edge goto (`return [3, N]`)
/// takes precedence when present, because it marks the end of the loop body.
/// Falls back to the first top-level return with an argument (the value return
/// after the loop). Skips jump returns nested inside if-blocks or try/catch,
/// which are internal control flow (continue/break), not loop boundaries.
fn find_loop_boundary(stmts: &[Stmt]) -> Option<usize> {
    for (i, stmt) in stmts.iter().enumerate() {
        if let Stmt::Return(_) = stmt {
            if return_jump_target(stmt).is_some() {
                return Some(i);
            }
        }
    }
    stmts
        .iter()
        .position(|stmt| return_value_stmt(stmt).is_some())
}

fn expr_stmt_expr(stmt: &Stmt) -> Option<Box<Expr>> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    Some(expr_stmt.expr.clone())
}

fn return_value_stmt(stmt: &Stmt) -> Option<&Stmt> {
    let Stmt::Return(ret) = stmt else {
        return None;
    };
    ret.arg.as_ref()?;
    Some(stmt)
}

fn convert_jump_returns(
    stmts: &mut [Stmt],
    break_target: usize,
    continue_target: usize,
) -> Option<bool> {
    let mut changed = false;
    for stmt in stmts {
        changed |= convert_jump_return(stmt, break_target, continue_target)?;
    }
    Some(changed)
}

fn convert_jump_return(
    stmt: &mut Stmt,
    break_target: usize,
    continue_target: usize,
) -> Option<bool> {
    match stmt {
        Stmt::Return(_) => {
            if let Some(target) = return_jump_target(stmt) {
                if target == break_target {
                    *stmt = Stmt::Break(BreakStmt {
                        span: DUMMY_SP,
                        label: None,
                    });
                } else if target == continue_target {
                    *stmt = Stmt::Continue(ContinueStmt {
                        span: DUMMY_SP,
                        label: None,
                    });
                } else {
                    return None;
                }
                return Some(true);
            }
            Some(false)
        }
        Stmt::If(if_stmt) => {
            let mut changed =
                convert_jump_return(&mut if_stmt.cons, break_target, continue_target)?;
            if let Some(alt) = &mut if_stmt.alt {
                changed |= convert_jump_return(alt, break_target, continue_target)?;
            }
            Some(changed)
        }
        Stmt::Block(block) => convert_jump_returns(&mut block.stmts, break_target, continue_target),
        Stmt::Try(try_stmt) => {
            let mut changed =
                convert_jump_returns(&mut try_stmt.block.stmts, break_target, continue_target)?;
            if let Some(handler) = &mut try_stmt.handler {
                changed |=
                    convert_jump_returns(&mut handler.body.stmts, break_target, continue_target)?;
            }
            if let Some(finalizer) = &mut try_stmt.finalizer {
                changed |= convert_jump_returns(
                    finalizer.stmts.as_mut_slice(),
                    break_target,
                    continue_target,
                )?;
            }
            Some(changed)
        }
        _ => Some(false),
    }
}

fn fold_state_temp_member_calls(state_name: &Atom, stmts: &mut Vec<Stmt>) {
    let mut folded = Vec::new();
    let mut index = 0usize;

    while index < stmts.len() {
        if let Some((stmt, consumed)) = try_fold_state_temp_member_call(state_name, &stmts[index..])
            .or_else(|| try_fold_local_temp_member_call(&stmts[index..]))
        {
            folded.push(stmt);
            index += consumed;
        } else {
            let mut stmt = stmts[index].clone();
            fold_state_temp_member_calls_in_stmt(state_name, &mut stmt);
            folded.push(stmt);
            index += 1;
        }
    }

    *stmts = folded;
}

fn fold_state_temp_member_calls_in_stmt(state_name: &Atom, stmt: &mut Stmt) {
    match stmt {
        Stmt::Block(block) => fold_state_temp_member_calls(state_name, &mut block.stmts),
        Stmt::For(for_stmt) => {
            if let Stmt::Block(block) = for_stmt.body.as_mut() {
                fold_state_temp_member_calls(state_name, &mut block.stmts);
            }
        }
        Stmt::Try(try_stmt) => {
            fold_state_temp_member_calls(state_name, &mut try_stmt.block.stmts);
            if let Some(handler) = &mut try_stmt.handler {
                fold_state_temp_member_calls(state_name, &mut handler.body.stmts);
            }
            if let Some(finalizer) = &mut try_stmt.finalizer {
                fold_state_temp_member_calls(state_name, &mut finalizer.stmts);
            }
        }
        Stmt::If(if_stmt) => {
            fold_state_temp_member_calls_in_stmt(state_name, &mut if_stmt.cons);
            if let Some(alt) = &mut if_stmt.alt {
                fold_state_temp_member_calls_in_stmt(state_name, alt);
            }
        }
        _ => {}
    }
}

fn try_fold_state_temp_member_call(state_name: &Atom, stmts: &[Stmt]) -> Option<(Stmt, usize)> {
    let (receiver_key, receiver) = extract_state_member_assign(state_name, stmts.first()?)?;
    let (arg_key, arg) = extract_state_member_assign(state_name, stmts.get(1)?)?;
    let call = extract_bound_state_member_call(
        state_name,
        stmts.get(2)?,
        &receiver_key,
        receiver,
        &arg_key,
        arg,
    )?;
    Some((
        Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(Expr::Call(call)),
        }),
        3,
    ))
}

fn try_fold_local_temp_member_call(stmts: &[Stmt]) -> Option<(Stmt, usize)> {
    let (receiver_key, receiver) = extract_local_temp_assign(stmts.first()?)?;
    let call = extract_bound_local_member_call(stmts.get(1)?, &receiver_key, receiver)?;
    if local_temp_read_before_reassign(&stmts[2..], &receiver_key) {
        return None;
    }
    Some((
        Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(Expr::Call(call)),
        }),
        2,
    ))
}

fn local_temp_read_before_reassign(stmts: &[Stmt], key: &BindingKey) -> bool {
    for stmt in stmts {
        if let Some(rhs) = direct_local_temp_reassign_rhs(stmt, key) {
            return expr_reads_local_temp(rhs, key);
        }
        if stmt_reads_local_temp(stmt, key) {
            return true;
        }
    }
    false
}

fn direct_local_temp_reassign_rhs<'a>(stmt: &'a Stmt, key: &BindingKey) -> Option<&'a Expr> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign || !assign_target_matches_local_temp(&assign.left, key) {
        return None;
    }
    Some(&assign.right)
}

fn stmt_reads_local_temp(stmt: &Stmt, key: &BindingKey) -> bool {
    let mut finder = LocalTempReadFinder { key, found: false };
    stmt.visit_with(&mut finder);
    finder.found
}

fn expr_reads_local_temp(expr: &Expr, key: &BindingKey) -> bool {
    let mut finder = LocalTempReadFinder { key, found: false };
    expr.visit_with(&mut finder);
    finder.found
}

struct LocalTempReadFinder<'a> {
    key: &'a BindingKey,
    found: bool,
}

impl Visit for LocalTempReadFinder<'_> {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        if let Some(init) = &decl.init {
            init.visit_with(self);
        }
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if !assign_target_matches_local_temp(&assign.left, self.key) {
            assign.left.visit_with(self);
        }
        assign.right.visit_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == self.key.0 && ident.ctxt == self.key.1 {
            self.found = true;
        }
    }
}

fn assign_target_matches_local_temp(target: &AssignTarget, key: &BindingKey) -> bool {
    matches!(
        target,
        AssignTarget::Simple(SimpleAssignTarget::Ident(binding))
            if binding.id.sym == key.0 && binding.id.ctxt == key.1
    )
}

fn extract_local_temp_assign(stmt: &Stmt) -> Option<(BindingKey, Box<Expr>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let ident = assign.left.as_simple()?.as_ident()?;
    if !is_likely_generated_alias(&ident.sym) {
        return None;
    }
    Some(((ident.sym.clone(), ident.ctxt), assign.right.clone()))
}

fn extract_bound_local_member_call(
    stmt: &Stmt,
    receiver_key: &BindingKey,
    receiver: Box<Expr>,
) -> Option<CallExpr> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let callee = call.callee.as_expr()?;
    let Expr::Member(call_member) = callee.as_ref() else {
        return None;
    };
    if !member_prop_name(&call_member.prop, "call") {
        return None;
    }
    let Expr::Member(method_member) = call_member.obj.as_ref() else {
        return None;
    };
    if local_temp_key(&method_member.obj).as_ref() != Some(receiver_key) {
        return None;
    }
    if call.args.is_empty() || local_temp_key(&call.args[0].expr).as_ref() != Some(receiver_key) {
        return None;
    }

    let mut next = call.clone();
    next.callee = Callee::Expr(Box::new(Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: receiver,
        prop: method_member.prop.clone(),
    })));
    next.args.remove(0);
    Some(next)
}

fn local_temp_key(expr: &Expr) -> Option<BindingKey> {
    let Expr::Ident(id) = strip_parens(expr) else {
        return None;
    };
    Some((id.sym.clone(), id.ctxt))
}

fn extract_state_member_assign(state_name: &Atom, stmt: &Stmt) -> Option<(Atom, Box<Expr>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let left_member = assign.left.as_simple().and_then(|s| s.as_member())?;
    if !is_ident_with_name(&left_member.obj, state_name) {
        return None;
    }
    let key = member_prop_atom(&left_member.prop)?;
    Some((key, assign.right.clone()))
}

fn extract_bound_state_member_call(
    state_name: &Atom,
    stmt: &Stmt,
    receiver_key: &Atom,
    receiver: Box<Expr>,
    arg_key: &Atom,
    arg: Box<Expr>,
) -> Option<CallExpr> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let callee = call.callee.as_expr()?;
    let Expr::Member(call_member) = callee.as_ref() else {
        return None;
    };
    if !member_prop_name(&call_member.prop, "call") {
        return None;
    }
    let Expr::Member(method_member) = call_member.obj.as_ref() else {
        return None;
    };
    if state_member_key(state_name, &method_member.obj).as_ref() != Some(receiver_key) {
        return None;
    }
    if call.args.len() != 2
        || state_member_key(state_name, &call.args[0].expr).as_ref() != Some(receiver_key)
        || state_member_key(state_name, &call.args[1].expr).as_ref() != Some(arg_key)
    {
        return None;
    }

    let mut next = call.clone();
    next.callee = Callee::Expr(Box::new(Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: receiver,
        prop: method_member.prop.clone(),
    })));
    next.args = vec![ExprOrSpread {
        spread: None,
        expr: arg,
    }];
    Some(next)
}

fn state_member_key(state_name: &Atom, expr: &Expr) -> Option<Atom> {
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    if !is_ident_with_name(&member.obj, state_name) {
        return None;
    }
    member_prop_atom(&member.prop)
}

// ============================================================
// Babel async arrow trampoline cleanup
// ============================================================

fn try_collapse_async_trampoline_iife(expr: &Expr) -> Option<Expr> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if !call.args.is_empty() {
        return None;
    }
    let callee = call.callee.as_expr()?;

    let body = match strip_parens(callee) {
        Expr::Fn(fn_expr) => fn_expr.function.body.as_ref()?,
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(block) => block,
            BlockStmtOrExpr::Expr(_) => return None,
        },
        _ => return None,
    };

    match body.stmts.as_slice() {
        [decl_stmt, return_stmt] => {
            let (binding, async_fn) = async_fn_binding_from_decl_stmt(decl_stmt)?;
            if !return_stmt_applies_binding(return_stmt, &binding) {
                return None;
            }
            Some(anonymous_async_fn_expr(async_fn))
        }
        [private_decl, public_decl, return_stmt] => {
            let (private_binding, async_fn) = async_fn_binding_from_decl_stmt(private_decl)?;
            let public_binding = forwarding_fn_decl_binding(public_decl, &private_binding)?;
            if !return_stmt_returns_binding(return_stmt, &public_binding) {
                return None;
            }
            Some(anonymous_async_fn_expr(async_fn))
        }
        _ => None,
    }
}

fn collapse_async_trampoline_iifes(module: &mut Module) {
    module.visit_mut_with(&mut AsyncTrampolineIifeCollapser);
}

struct AsyncTrampolineIifeCollapser;

impl VisitMut for AsyncTrampolineIifeCollapser {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        if let Some(collapsed) = try_collapse_async_trampoline_iife(expr) {
            *expr = collapsed;
        }
    }
}

fn collapse_async_trampoline_sequences(module: &mut Module) {
    let binding_ref_counts = collect_binding_ref_counts(module);
    let mut collapser = AsyncTrampolineSequenceCollapser { binding_ref_counts };
    module.visit_mut_with(&mut collapser);
}

struct AsyncTrampolineSequenceCollapser {
    binding_ref_counts: HashMap<BindingKey, usize>,
}

impl VisitMut for AsyncTrampolineSequenceCollapser {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        if let Some(collapsed) =
            try_collapse_async_trampoline_sequence(expr, &self.binding_ref_counts)
        {
            *expr = collapsed;
        }
    }
}

fn try_collapse_async_trampoline_sequence(
    expr: &Expr,
    binding_ref_counts: &HashMap<BindingKey, usize>,
) -> Option<Expr> {
    let Expr::Seq(seq) = expr else {
        return None;
    };
    let [assignment, wrapper] = seq.exprs.as_slice() else {
        return None;
    };

    let (binding, async_fn) = async_fn_assignment_from_expr(assignment)?;
    if binding_ref_counts.get(&binding).copied() != Some(2) {
        return None;
    }
    if !expr_applies_binding(wrapper, &binding) {
        return None;
    }

    Some(anonymous_async_fn_expr(async_fn))
}

fn collapse_async_trampoline_assignments(module: &mut Module) {
    let mut index = 0;
    while index + 1 < module.body.len() {
        let Some((binding, async_fn)) = async_fn_assignment_from_item(&module.body[index]) else {
            index += 1;
            continue;
        };
        if !var_item_init_applies_binding(&module.body[index + 1], &binding) {
            index += 1;
            continue;
        }
        if binding_used_outside_pair(&module.body, index, index + 1, &binding) {
            index += 1;
            continue;
        }

        if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &mut module.body[index + 1] {
            for decl in &mut var.decls {
                if let Some(init) = &decl.init {
                    if expr_applies_binding(init, &binding) {
                        decl.init = Some(Box::new(anonymous_async_fn_expr(async_fn.clone())));
                    }
                }
            }
        }
        module.body.remove(index);
    }
}

fn async_fn_assignment_from_expr(expr: &Expr) -> Option<(BindingKey, FnExpr)> {
    let Expr::Assign(assign) = expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left else {
        return None;
    };
    let async_fn = async_fn_from_expr(&assign.right)?;
    Some(((left.id.sym.clone(), left.id.ctxt), async_fn.clone()))
}

fn async_fn_binding_from_decl_stmt(stmt: &Stmt) -> Option<(BindingKey, FnExpr)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let async_fn = async_fn_from_expr(decl.init.as_deref()?)?;
    Some(((binding.id.sym.clone(), binding.id.ctxt), async_fn.clone()))
}

fn async_fn_assignment_from_item(item: &ModuleItem) -> Option<(BindingKey, FnExpr)> {
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left else {
        return None;
    };
    let async_fn = async_fn_from_expr(&assign.right)?;
    Some(((left.id.sym.clone(), left.id.ctxt), async_fn.clone()))
}

fn async_fn_from_expr(expr: &Expr) -> Option<&FnExpr> {
    let Expr::Fn(fn_expr) = expr else {
        return None;
    };
    if fn_expr.function.is_async && !fn_expr.function.is_generator {
        Some(fn_expr)
    } else {
        None
    }
}

fn return_stmt_applies_binding(stmt: &Stmt, binding: &BindingKey) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = ret.arg.as_deref() else {
        return false;
    };
    expr_applies_binding(arg, binding)
}

fn return_stmt_returns_binding(stmt: &Stmt, binding: &BindingKey) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(Expr::Ident(id)) = ret.arg.as_deref() else {
        return false;
    };
    id.sym == binding.0 && id.ctxt == binding.1
}

fn forwarding_fn_decl_binding(stmt: &Stmt, target: &BindingKey) -> Option<BindingKey> {
    let Stmt::Decl(Decl::Fn(fn_decl)) = stmt else {
        return None;
    };
    let body = fn_decl.function.body.as_ref()?;
    let [return_stmt] = body.stmts.as_slice() else {
        return None;
    };
    if !return_stmt_applies_binding(return_stmt, target) {
        return None;
    }
    Some((fn_decl.ident.sym.clone(), fn_decl.ident.ctxt))
}

fn var_item_init_applies_binding(item: &ModuleItem, binding: &BindingKey) -> bool {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return false;
    };
    var.decls.iter().any(|decl| {
        decl.init
            .as_deref()
            .is_some_and(|init| expr_applies_binding(init, binding))
    })
}

fn expr_applies_binding(expr: &Expr, binding: &BindingKey) -> bool {
    if extract_apply_this_arguments_callee(expr)
        .is_some_and(|id| id.sym == binding.0 && id.ctxt == binding.1)
    {
        return true;
    }

    let Expr::Fn(fn_expr) = expr else {
        return false;
    };
    let Some(body) = &fn_expr.function.body else {
        return false;
    };
    let [stmt] = body.stmts.as_slice() else {
        return false;
    };
    return_stmt_applies_binding(stmt, binding)
}

fn anonymous_async_fn_expr(mut fn_expr: FnExpr) -> Expr {
    fn_expr.ident = None;
    Expr::Fn(fn_expr)
}

fn collect_binding_ref_counts(module: &Module) -> HashMap<BindingKey, usize> {
    let mut counter = BindingRefCounter {
        counts: HashMap::new(),
    };
    module.visit_with(&mut counter);
    counter.counts
}

struct BindingRefCounter {
    counts: HashMap<BindingKey, usize>,
}

impl Visit for BindingRefCounter {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        if let Some(init) = &decl.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, fn_decl: &FnDecl) {
        fn_decl.function.visit_with(self);
    }

    fn visit_fn_expr(&mut self, fn_expr: &FnExpr) {
        fn_expr.function.visit_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        *self
            .counts
            .entry((ident.sym.clone(), ident.ctxt))
            .or_default() += 1;
    }
}

// ============================================================
// esbuild __async → async function
// ============================================================

/// Strips a `_regeneratorValues(...)` wrapper from `yield*` delegations whose
/// callee is a detected values-helper binding (post-decode, mangle-safe).
struct RegeneratorValuesUnwrapper<'a> {
    helpers: &'a [BindingKey],
}

impl VisitMut for RegeneratorValuesUnwrapper<'_> {
    fn visit_mut_yield_expr(&mut self, yield_expr: &mut YieldExpr) {
        yield_expr.visit_mut_children_with(self);
        if !yield_expr.delegate {
            return;
        }
        if let Some(arg) = yield_expr.arg.take() {
            yield_expr.arg = Some(unwrap_regenerator_values(arg, self.helpers));
        }
    }
}

/// Collect top-level `_regeneratorValues` helper bindings by canonical name or
/// by body shape (single iterable param, `Symbol.iterator` / `@@iterator`
/// lookup, `TypeError` on non-iterable), which survives minification.
fn collect_regenerator_values_helpers(module: &Module) -> Vec<BindingKey> {
    let mut helpers = Vec::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                let name = fn_decl.ident.sym.as_ref();
                if (name.contains("regeneratorValues") || is_likely_generated_alias(name))
                    && fn_is_regenerator_values_helper(&fn_decl.function)
                {
                    helpers.push(binding_key(&fn_decl.ident));
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    let Pat::Ident(binding) = &decl.name else {
                        continue;
                    };
                    let name = binding.id.sym.as_ref();
                    if !name.contains("regeneratorValues") && !is_likely_generated_alias(name) {
                        continue;
                    }
                    if decl
                        .init
                        .as_deref()
                        .is_some_and(is_regenerator_values_helper_expr)
                    {
                        helpers.push(binding_key(&binding.id));
                    }
                }
            }
            _ => {}
        }
    }
    helpers
}

fn is_regenerator_values_helper_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Fn(fn_expr) => fn_is_regenerator_values_helper(&fn_expr.function),
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 1 {
                return false;
            }
            let mut finder = RegeneratorValuesHelperFinder::default();
            arrow.body.visit_with(&mut finder);
            finder.has_shape()
        }
        _ => false,
    }
}

fn fn_is_regenerator_values_helper(function: &Function) -> bool {
    if function.params.len() != 1 {
        return false;
    }
    let Some(body) = &function.body else {
        return false;
    };
    let mut finder = RegeneratorValuesHelperFinder::default();
    body.visit_with(&mut finder);
    finder.has_shape()
}

#[derive(Default)]
struct RegeneratorValuesHelperFinder {
    found_symbol_iterator: bool,
    found_at_iterator: bool,
    found_type_error: bool,
}

impl RegeneratorValuesHelperFinder {
    fn has_shape(&self) -> bool {
        self.found_symbol_iterator && self.found_at_iterator && self.found_type_error
    }
}

impl Visit for RegeneratorValuesHelperFinder {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if let Expr::Ident(obj) = member.obj.as_ref() {
            if obj.sym.as_ref() == "Symbol" && member_prop_name(&member.prop, "iterator") {
                self.found_symbol_iterator = true;
            }
        }
        member.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym.as_ref() == "TypeError" {
            self.found_type_error = true;
        }
    }

    fn visit_lit(&mut self, lit: &Lit) {
        if let Lit::Str(str_lit) = lit {
            if str_lit.value.as_str() == Some("@@iterator") {
                self.found_at_iterator = true;
            }
        }
    }
}

fn collect_esbuild_yield_star_helpers(module: &Module) -> Vec<BindingKey> {
    module
        .body
        .iter()
        .flat_map(|item| {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                return Vec::new();
            };
            var.decls
                .iter()
                .filter_map(|decl| {
                    let Pat::Ident(binding) = &decl.name else {
                        return None;
                    };
                    let name = binding.id.sym.as_ref();
                    if name != "__yieldStar" && !is_likely_generated_alias(name) {
                        return None;
                    }
                    let init = decl.init.as_deref()?;
                    if is_esbuild_yield_star_helper_expr(init) {
                        Some((binding.id.sym.clone(), binding.id.ctxt))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn is_esbuild_yield_star_helper_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Arrow(arrow) => {
            let mut finder = EsbuildYieldStarHelperFinder::default();
            arrow.body.visit_with(&mut finder);
            finder.has_shape()
        }
        Expr::Fn(fn_expr) => {
            let Some(body) = &fn_expr.function.body else {
                return false;
            };
            let mut finder = EsbuildYieldStarHelperFinder::default();
            body.visit_with(&mut finder);
            finder.has_shape()
        }
        _ => false,
    }
}

#[derive(Default)]
struct EsbuildYieldStarHelperFinder {
    found_async_iterator: bool,
    found_iterator: bool,
    found_await_wrapper: bool,
}

impl EsbuildYieldStarHelperFinder {
    fn has_shape(&self) -> bool {
        self.found_async_iterator && self.found_iterator && self.found_await_wrapper
    }
}

impl Visit for EsbuildYieldStarHelperFinder {
    fn visit_lit(&mut self, lit: &Lit) {
        if let Lit::Str(str_lit) = lit {
            if str_lit.value.as_str() == Some("asyncIterator") {
                self.found_async_iterator = true;
            }
            if str_lit.value.as_str() == Some("iterator") {
                self.found_iterator = true;
            }
        }
    }

    fn visit_new_expr(&mut self, new_expr: &swc_core::ecma::ast::NewExpr) {
        if new_expr
            .args
            .as_ref()
            .is_some_and(|args| args.len() == 2 && is_number_lit(&args[1].expr, 1.0))
        {
            self.found_await_wrapper = true;
        }
        new_expr.visit_children_with(self);
    }
}

fn unwrap_esbuild_yield_star_arg(
    expr: &Expr,
    esbuild_yield_star_helpers: &[BindingKey],
) -> Option<Box<Expr>> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    if call.args.len() != 1 {
        return None;
    }
    let callee = call.callee.as_expr()?;
    let Expr::Ident(id) = callee.as_ref() else {
        return None;
    };
    if !esbuild_yield_star_helpers
        .iter()
        .any(|(sym, ctxt)| id.sym == *sym && id.ctxt == *ctxt)
    {
        return None;
    }
    Some(call.args[0].expr.clone())
}

fn collect_esbuild_async_helpers(module: &Module, unresolved_mark: Mark) -> Vec<BindingKey> {
    module
        .body
        .iter()
        .flat_map(|item| {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                return Vec::new();
            };
            var.decls
                .iter()
                .filter_map(|decl| {
                    let Pat::Ident(binding) = &decl.name else {
                        return None;
                    };
                    let name = binding.id.sym.as_ref();
                    if name != "__async" && !is_likely_generated_alias(name) {
                        return None;
                    }
                    let init = decl.init.as_deref()?;
                    if is_esbuild_async_helper_expr(init, unresolved_mark) {
                        Some((binding.id.sym.clone(), binding.id.ctxt))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn is_esbuild_async_helper_expr(expr: &Expr, unresolved_mark: Mark) -> bool {
    let body = match expr {
        Expr::Arrow(arrow) => arrow.body.as_ref(),
        Expr::Fn(fn_expr) => {
            let Some(body) = &fn_expr.function.body else {
                return false;
            };
            return esbuild_async_helper_body_has_shape(&body.stmts, unresolved_mark);
        }
        _ => return false,
    };

    match body {
        BlockStmtOrExpr::BlockStmt(block) => {
            esbuild_async_helper_body_has_shape(&block.stmts, unresolved_mark)
        }
        BlockStmtOrExpr::Expr(expr) => {
            let mut finder = EsbuildAsyncHelperFinder::new(unresolved_mark);
            expr.visit_with(&mut finder);
            finder.found_promise && finder.found_generator_apply
        }
    }
}

fn esbuild_async_helper_body_has_shape(stmts: &[Stmt], unresolved_mark: Mark) -> bool {
    let mut finder = EsbuildAsyncHelperFinder::new(unresolved_mark);
    for stmt in stmts {
        stmt.visit_with(&mut finder);
    }
    finder.found_promise && finder.found_generator_apply
}

struct EsbuildAsyncHelperFinder {
    unresolved_mark: Mark,
    found_promise: bool,
    found_generator_apply: bool,
}

impl EsbuildAsyncHelperFinder {
    fn new(unresolved_mark: Mark) -> Self {
        Self {
            unresolved_mark,
            found_promise: false,
            found_generator_apply: false,
        }
    }
}

impl Visit for EsbuildAsyncHelperFinder {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::New(new_expr) = expr {
            if matches!(
                new_expr.callee.as_ref(),
                Expr::Ident(id)
                    if id.sym.as_ref() == "Promise" && id.ctxt.outer() == self.unresolved_mark
            ) {
                self.found_promise = true;
            }
        }

        if let Expr::Call(call) = expr {
            if let Some(callee) = call.callee.as_expr() {
                if let Expr::Member(member) = callee.as_ref() {
                    if member_prop_name(&member.prop, "apply") {
                        self.found_generator_apply = true;
                    }
                }
            }
        }

        expr.visit_children_with(self);
    }
}

fn try_transform_esbuild_async_function(
    body: &mut BlockStmt,
    esbuild_async_helpers: &[BindingKey],
) -> bool {
    if body.stmts.len() != 1 {
        return false;
    }
    let Stmt::Return(ret) = &body.stmts[0] else {
        return false;
    };
    let Some(arg) = ret.arg.as_deref() else {
        return false;
    };
    let Some(mut stmts) = extract_esbuild_async_call_body(arg, esbuild_async_helpers) else {
        return false;
    };
    replace_yield_with_await(&mut stmts);
    body.stmts = stmts;
    true
}

fn try_transform_esbuild_async_arrow(
    arrow: &mut ArrowExpr,
    esbuild_async_helpers: &[BindingKey],
) -> bool {
    let Some(mut stmts) = (match arrow.body.as_ref() {
        BlockStmtOrExpr::Expr(expr) => extract_esbuild_async_call_body(expr, esbuild_async_helpers),
        BlockStmtOrExpr::BlockStmt(block) if block.stmts.len() == 1 => {
            let Stmt::Return(ret) = &block.stmts[0] else {
                return false;
            };
            let Some(arg) = ret.arg.as_deref() else {
                return false;
            };
            extract_esbuild_async_call_body(arg, esbuild_async_helpers)
        }
        _ => None,
    }) else {
        return false;
    };

    replace_yield_with_await(&mut stmts);
    arrow.is_async = true;
    *arrow.body = BlockStmtOrExpr::BlockStmt(BlockStmt {
        span: DUMMY_SP,
        ctxt: Default::default(),
        stmts,
    });
    true
}

fn extract_esbuild_async_call_body(
    expr: &Expr,
    esbuild_async_helpers: &[BindingKey],
) -> Option<Vec<Stmt>> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if call.args.len() != 3 {
        return None;
    }
    let callee = call.callee.as_expr()?;
    if !is_esbuild_async_callee(callee, esbuild_async_helpers) {
        return None;
    }
    if !is_esbuild_async_this_arg(&call.args[0].expr)
        || !is_esbuild_async_arguments_arg(&call.args[1].expr)
    {
        return None;
    }

    let Expr::Fn(fn_expr) = call.args[2].expr.as_ref() else {
        return None;
    };
    if !fn_expr.function.is_generator || !fn_expr.function.params.is_empty() {
        return None;
    }
    Some(fn_expr.function.body.as_ref()?.stmts.clone())
}

fn is_esbuild_async_this_arg(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::This(_) | Expr::Lit(Lit::Null(_)))
}

fn is_esbuild_async_arguments_arg(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Ident(id) => id.sym.as_ref() == "arguments",
        Expr::Lit(Lit::Null(_)) => true,
        _ => false,
    }
}

fn is_esbuild_async_callee(expr: &Expr, esbuild_async_helpers: &[BindingKey]) -> bool {
    let Expr::Ident(id) = expr else {
        return false;
    };
    esbuild_async_helpers
        .iter()
        .any(|(sym, ctxt)| id.sym == *sym && id.ctxt == *ctxt)
}

// ============================================================
// _asyncToGenerator → async function
// ============================================================

fn is_paramless_async_to_gen_iife(expr: &Expr, async_to_gen_callees: &AsyncToGenCallees) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    if call.args.len() != 1 {
        return false;
    }
    let Some(callee) = call.callee.as_expr() else {
        return false;
    };
    if !is_async_to_gen_callee(callee, async_to_gen_callees) {
        return false;
    }
    match call.args[0].expr.as_ref() {
        Expr::Fn(fn_expr) => fn_expr.function.params.is_empty(),
        Expr::Call(mark_call) => {
            let Some(callee_expr) = mark_call.callee.as_expr() else {
                return false;
            };
            let Expr::Member(member) = callee_expr.as_ref() else {
                return false;
            };
            if !is_mark_prop(&member.prop) || mark_call.args.len() != 1 {
                return false;
            }
            matches!(mark_call.args[0].expr.as_ref(), Expr::Fn(fn_expr) if fn_expr.function.params.is_empty())
        }
        _ => false,
    }
}

fn try_transform_async_to_generator_expr(
    expr: Expr,
    async_to_gen_callees: &AsyncToGenCallees,
    generator_helpers: &[BindingKey],
) -> Option<(Expr, Option<BindingKey>)> {
    let Expr::Call(mut call) = expr else {
        return None;
    };
    if call.args.len() != 1 {
        return None;
    }
    let callee = call.callee.as_expr()?;
    if !is_async_to_gen_callee(callee, async_to_gen_callees) {
        return None;
    }

    let gen_fn_arg = *call.args.remove(0).expr;
    let (mut fn_expr, mark_key) = build_async_fn_expr_from_gen_arg(gen_fn_arg, generator_helpers)?;
    let body = fn_expr.function.body.as_mut()?;
    replace_yield_with_await(&mut body.stmts);
    fn_expr.function.is_async = true;
    fn_expr.function.is_generator = false;
    Some((Expr::Fn(fn_expr), mark_key))
}

fn build_async_fn_expr_from_gen_arg(
    gen_fn_arg: Expr,
    generator_helpers: &[BindingKey],
) -> Option<(FnExpr, Option<BindingKey>)> {
    match gen_fn_arg {
        Expr::Fn(fn_expr) => {
            let mut function = *fn_expr.function;
            let mark_key = if function.is_generator {
                None
            } else {
                let body = function.body.as_mut()?;
                if let Some(mark_key) = try_transform_regenerator_wrap(body) {
                    mark_key
                } else if try_transform_ts_generator_body(body, generator_helpers) {
                    None
                } else {
                    return None;
                }
            };
            Some((
                FnExpr {
                    ident: fn_expr.ident,
                    function: Box::new(function),
                },
                mark_key,
            ))
        }
        Expr::Call(mark_call) => {
            let callee_expr = mark_call.callee.as_expr()?;
            let Expr::Member(member) = callee_expr.as_ref() else {
                return None;
            };
            if !is_mark_prop(&member.prop) || mark_call.args.len() != 1 {
                return None;
            }
            let Expr::Fn(fn_expr) = *mark_call.args.into_iter().next()?.expr else {
                return None;
            };
            let mut function = *fn_expr.function;
            let mark_key = if function.is_generator {
                None
            } else {
                let body = function.body.as_mut()?;
                if let Some(mark_key) = try_transform_regenerator_wrap(body) {
                    mark_key
                } else if try_transform_ts_generator_body(body, generator_helpers) {
                    None
                } else {
                    return None;
                }
            };
            Some((
                FnExpr {
                    ident: fn_expr.ident,
                    function: Box::new(function),
                },
                mark_key,
            ))
        }
        _ => None,
    }
}

fn try_transform_async_to_generator(
    body: &mut BlockStmt,
    async_to_gen_callees: &AsyncToGenCallees,
    generator_helpers: &[BindingKey],
    _unresolved_mark: Mark,
) -> bool {
    let return_idx = body
        .stmts
        .iter()
        .position(|s| is_async_to_gen_return(s, async_to_gen_callees));
    let return_idx = match return_idx {
        Some(i) => i,
        None => return false,
    };

    // Pre-check: validate the pattern is extractable before removing the stmt.
    if !can_extract_async_to_gen(&body.stmts[return_idx], generator_helpers) {
        return false;
    }

    let ret_stmt = body.stmts.remove(return_idx);
    let inner_stmts =
        match extract_async_to_gen_body(ret_stmt, async_to_gen_callees, generator_helpers) {
            Some(s) => s,
            None => unreachable!("can_extract_async_to_gen passed but extract failed"),
        };

    let mut inner_stmts = inner_stmts;
    replace_yield_with_await(&mut inner_stmts);

    body.stmts.splice(return_idx..return_idx, inner_stmts);
    true
}

fn is_async_to_gen_return(stmt: &Stmt, async_to_gen_callees: &AsyncToGenCallees) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = &ret.arg else { return false };
    is_async_to_gen_call(arg, async_to_gen_callees)
}

/// Non-destructive check: can we extract the async body from this statement?
/// Validates the same conditions as extract_async_to_gen_body without consuming the AST.
fn can_extract_async_to_gen(stmt: &Stmt, generator_helpers: &[BindingKey]) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = &ret.arg else { return false };
    let Expr::Call(outer_call) = arg.as_ref() else {
        return false;
    };
    // Outer IIFE must have no arguments
    if !outer_call.args.is_empty() {
        return false;
    }
    let Some(outer_callee) = outer_call.callee.as_expr() else {
        return false;
    };
    let Expr::Call(inner_call) = outer_callee.as_ref() else {
        return false;
    };
    if inner_call.args.len() != 1 {
        return false;
    }
    let gen_fn = &inner_call.args[0].expr;
    match gen_fn.as_ref() {
        Expr::Fn(fn_expr) => {
            // Inner generator must have no params
            if !fn_expr.function.params.is_empty() {
                return false;
            }
            if fn_expr.function.is_generator {
                return true;
            }
            // Non-generator: must contain either regenerator.wrap or SWC's
            // _ts_generator state machine.
            fn_expr.function.body.as_ref().is_some_and(|body| {
                body.stmts
                    .iter()
                    .any(|s| is_regenerator_wrap_return(s) && !has_nested_control_flow_in_stmt(s))
                    || {
                        let mut body = body.clone();
                        try_transform_ts_generator_body(&mut body, generator_helpers)
                    }
            })
        }
        Expr::Call(mark_call) => {
            // regeneratorRuntime.mark(function _callee() { ... })
            let Some(callee) = mark_call.callee.as_expr() else {
                return false;
            };
            let Expr::Member(member) = callee.as_ref() else {
                return false;
            };
            if !is_mark_prop(&member.prop) || mark_call.args.len() != 1 {
                return false;
            }
            let Expr::Fn(fn_expr) = mark_call.args[0].expr.as_ref() else {
                return false;
            };
            if !fn_expr.function.params.is_empty() {
                return false;
            }
            fn_expr.function.body.as_ref().is_some_and(|body| {
                body.stmts
                    .iter()
                    .any(|s| is_regenerator_wrap_return(s) && !has_nested_control_flow_in_stmt(s))
                    || {
                        let mut body = body.clone();
                        try_transform_ts_generator_body(&mut body, generator_helpers)
                    }
            })
        }
        _ => false,
    }
}

/// Check for `_asyncToGenerator(fn)()` — IIFE pattern with scope-aware matching
fn is_async_to_gen_call(expr: &Expr, async_to_gen_callees: &AsyncToGenCallees) -> bool {
    let Expr::Call(outer_call) = expr else {
        return false;
    };
    let Some(outer_callee) = outer_call.callee.as_expr() else {
        return false;
    };
    let Expr::Call(inner_call) = outer_callee.as_ref() else {
        return false;
    };
    let Some(inner_callee) = inner_call.callee.as_expr() else {
        return false;
    };
    is_async_to_gen_callee(inner_callee, async_to_gen_callees)
}

fn is_async_to_gen_callee(expr: &Expr, async_to_gen_callees: &AsyncToGenCallees) -> bool {
    if let Expr::Ident(id) = expr {
        return async_to_gen_callees
            .direct
            .iter()
            .any(|(sym, ctxt)| id.sym == *sym && id.ctxt == *ctxt);
    }

    let Expr::Member(member) = expr else {
        return false;
    };
    if !member_prop_name(&member.prop, "default") {
        return false;
    }
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    async_to_gen_callees
        .default_members
        .iter()
        .any(|(sym, ctxt)| obj.sym == *sym && obj.ctxt == *ctxt)
}

fn extract_async_to_gen_body(
    stmt: Stmt,
    async_to_gen_callees: &AsyncToGenCallees,
    generator_helpers: &[BindingKey],
) -> Option<Vec<Stmt>> {
    let Stmt::Return(ret) = stmt else { return None };
    let arg = *ret.arg?;
    // _asyncToGenerator(fn)()
    let Expr::Call(outer_call) = arg else {
        return None;
    };
    // P1-2: Bail out if the outer IIFE call has arguments
    if !outer_call.args.is_empty() {
        return None;
    }
    let Expr::Call(mut inner_call) = *outer_call.callee.expect_expr() else {
        return None;
    };
    let inner_callee = inner_call.callee.as_expr()?;
    if !is_async_to_gen_callee(inner_callee, async_to_gen_callees) {
        return None;
    }
    if inner_call.args.len() != 1 {
        return None;
    }

    let gen_fn_arg = *inner_call.args.remove(0).expr;

    // The argument could be:
    // 1. function*() { ... } — native generator
    // 2. regeneratorRuntime.mark(function _callee() { ... }) — babel wrapped
    match gen_fn_arg {
        Expr::Fn(fn_expr) => {
            // P1-2: Bail out if inner generator has params — real Babel output
            // never has params here (they're on the outer function via closure)
            if !fn_expr.function.params.is_empty() {
                return None;
            }
            if fn_expr.function.is_generator {
                // Native generator: just extract body
                return fn_expr.function.body.map(|b| b.stmts);
            }
            // Non-generator function that contains regeneratorRuntime.wrap
            let mut body = fn_expr.function.body?;
            if try_transform_regenerator_wrap(&mut body).is_some()
                || try_transform_ts_generator_body(&mut body, generator_helpers)
            {
                return Some(body.stmts);
            }
            None
        }
        Expr::Call(mark_call) => {
            // regeneratorRuntime.mark(function _callee() { ... })
            let callee_expr = mark_call.callee.as_expr()?;
            let Expr::Member(member) = callee_expr.as_ref() else {
                return None;
            };
            if !is_mark_prop(&member.prop) {
                return None;
            }
            if mark_call.args.len() != 1 {
                return None;
            }
            let inner_fn = *mark_call.args.into_iter().next()?.expr;
            let Expr::Fn(fn_expr) = inner_fn else {
                return None;
            };
            let mut body = fn_expr.function.body?;
            if try_transform_regenerator_wrap(&mut body).is_some()
                || try_transform_ts_generator_body(&mut body, generator_helpers)
            {
                return Some(body.stmts);
            }
            None
        }
        _ => None,
    }
}

fn replace_yield_with_await(stmts: &mut Vec<Stmt>) {
    struct YieldToAwait;
    impl VisitMut for YieldToAwait {
        fn visit_mut_function(&mut self, _func: &mut Function) {}

        fn visit_mut_arrow_expr(&mut self, _arrow: &mut ArrowExpr) {}

        fn visit_mut_expr(&mut self, expr: &mut Expr) {
            if let Expr::Yield(y) = expr {
                let arg = y.arg.take().unwrap_or_else(|| {
                    Box::new(Expr::Ident(Ident::new_no_ctxt(
                        "undefined".into(),
                        DUMMY_SP,
                    )))
                });
                *expr = Expr::Await(AwaitExpr {
                    span: DUMMY_SP,
                    arg,
                });
                expr.visit_mut_children_with(self);
                return;
            }
            expr.visit_mut_children_with(self);
        }
    }
    let mut v = YieldToAwait;
    for s in stmts.iter_mut() {
        s.visit_mut_with(&mut v);
    }
}

// ============================================================
// Module-level cleanup: remove regeneratorRuntime.mark() decls
// ============================================================

/// Remove only the mark declarations whose bindings were consumed by
/// successful `.wrap()` transforms. Only removes `var x = <expr>.mark(fn)`
/// where `(x.sym, x.ctxt)` matches a consumed mark key.
fn remove_consumed_mark_declarations(module: &mut Module, consumed_marks: &[BindingKey]) {
    if consumed_marks.is_empty() {
        return;
    }
    module.body.retain_mut(|item| {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            return true;
        };
        var.decls.retain(|decl| {
            let Pat::Ident(bi) = &decl.name else {
                return true;
            };
            if !consumed_marks
                .iter()
                .any(|(sym, ctxt)| bi.id.sym == *sym && bi.id.ctxt == *ctxt)
            {
                return true;
            }
            // Only remove if the initializer is a .mark() call
            let Some(init) = &decl.init else {
                return true;
            };
            let Expr::Call(call) = init.as_ref() else {
                return true;
            };
            let Some(callee) = call.callee.as_expr() else {
                return true;
            };
            let Expr::Member(member) = callee.as_ref() else {
                return true;
            };
            !is_mark_prop(&member.prop)
        });
        !var.decls.is_empty()
    });
}

fn remove_helper_decls(module: &mut Module, to_remove: &[BindingKey]) {
    let removable: HashSet<BindingKey> = to_remove.iter().cloned().collect();
    remove_fn_decls_by_binding(module, &removable);
    remove_var_declarators_by_binding(&mut module.body, &removable);
}

fn remove_unused_helper_decls(module: &mut Module, helpers: &[BindingKey]) {
    if helpers.is_empty() {
        return;
    }
    let unused: Vec<_> = helpers
        .iter()
        .filter(|helper| count_binding_refs(module, helper) <= 1)
        .cloned()
        .collect();
    remove_helper_decls(module, &unused);
}

// ============================================================
// Shared helpers
// ============================================================

fn is_ident_with_name(expr: &Expr, name: &Atom) -> bool {
    matches!(expr, Expr::Ident(id) if id.sym == *name)
}

fn member_prop_atom(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(id) => Some(id.sym.clone()),
        MemberProp::Computed(c) => {
            if let Expr::Lit(Lit::Str(s)) = c.expr.as_ref() {
                s.value.as_str().map(Atom::from)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_number_lit(expr: &Expr, expected: f64) -> bool {
    matches!(strip_parens(expr), Expr::Lit(Lit::Num(num)) if num.value == expected)
}

fn number_lit_usize(expr: &Expr) -> Option<usize> {
    let Expr::Lit(Lit::Num(num)) = strip_parens(expr) else {
        return None;
    };
    Some(num.value as usize)
}

fn export_name_is(name: &swc_core::ecma::ast::ModuleExportName, expected: &str) -> bool {
    match name {
        swc_core::ecma::ast::ModuleExportName::Ident(id) => id.sym.as_ref() == expected,
        swc_core::ecma::ast::ModuleExportName::Str(s) => s.value.as_str() == Some(expected),
    }
}

fn str_to_atom(value: &swc_core::atoms::Wtf8Atom) -> Atom {
    Atom::from(value.as_str().unwrap_or(""))
}
