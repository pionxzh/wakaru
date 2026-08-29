use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, CallExpr, Callee, Class, Decl, Expr, ForHead,
    ForInStmt, ForOfStmt, Function, Lit, MemberProp, Module, ModuleItem, NewExpr, ObjectPatProp,
    OptChainBase, OptChainExpr, Pat, SimpleAssignTarget, Stmt, TaggedTpl, UpdateExpr, VarDeclKind,
    VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::transpiler_helper_utils::{
    classify_inline_helper_call, remove_helper_declarations, BindingKey, LocalHelperContext,
    TranspilerHelperKind,
};

/// Detects and unwraps `interopRequireDefault` helper calls.
///
/// Transforms:
///   `var _a = _interopRequireDefault(require("a")); _a.default`
///   → `var _a = require("a"); _a`
pub struct UnInteropRequireDefault;

impl UnInteropRequireDefault {
    pub(crate) fn run_with_helpers(module: &mut Module, local_helpers: &LocalHelperContext) {
        run_un_interop_require_default(module, local_helpers);
    }
}

impl VisitMut for UnInteropRequireDefault {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_interop_require_default(module, &local_helpers);
    }
}

fn run_un_interop_require_default(module: &mut Module, local_helpers: &LocalHelperContext) {
    let mut affected_bindings: HashSet<BindingKey> = HashSet::new();
    let mut preserve_named_helpers = false;

    // --- Named helper path ---
    let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::InteropRequireDefault);
    let tslib_namespaces = local_helpers.tslib_namespaces();
    let has_direct_tslib_calls =
        local_helpers.has_tslib_require_member_call(TranspilerHelperKind::InteropRequireDefault);

    if !helpers.is_empty() || !tslib_namespaces.is_empty() || has_direct_tslib_calls {
        // Phase 1: Collect which bindings receive helper-wrapped values
        let mut collector = AffectedBindingCollector {
            local_helpers,
            affected: &mut affected_bindings,
        };
        collector.visit_module(module);

        // SWC's AMD output receives dependencies as factory parameters and
        // then wraps them in standalone assignments:
        //
        //   react = interopRequireDefault(react);
        //
        // AMD extraction turns the parameter into a require-backed local. The
        // helper call is therefore the binding's generated initialization,
        // not a later semantic reassignment. Recognize only the exact,
        // unconditional top-level producer shape and remove it before the
        // ordinary reassignment check.
        let assignment_initializers =
            collect_assignment_form_initializers(module, local_helpers, &mut affected_bindings);
        if !assignment_initializers.is_empty() {
            module.body = std::mem::take(&mut module.body)
                .into_iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    (!assignment_initializers.contains(&index)).then_some(item)
                })
                .collect();
        }

        // Phase 2a: Unwrap helper calls — replace `helper(arg)` with `arg`.
        let mut call_unwrapper = CallUnwrapper {
            local_helpers,
            preserved_assignment_form: false,
        };
        module.visit_mut_with(&mut call_unwrapper);

        // A preserved call still needs its helper declaration/import. Keeping
        // every same-kind helper is conservative and avoids guessing which
        // generated alias the retained call reaches.
        preserve_named_helpers = call_unwrapper.preserved_assignment_form;
    }

    // --- Inline IIFE interop path ---
    // Detect: `const x = ((e) => { if (e && e.__esModule) return e; return {default: e} })(require(...))`
    // Replace with: `const x = require(...)`  and record `x` as affected
    unwrap_inline_interop_iifes(module, &mut affected_bindings);

    // Phase 2b: Rewrite `.default` member access on affected bindings,
    //           but only if the binding is never reassigned.
    if !affected_bindings.is_empty() {
        let mut reassigned = HashSet::new();
        let mut checker = ReassignmentChecker {
            candidates: &affected_bindings,
            reassigned: &mut reassigned,
        };
        module.visit_with(&mut checker);
        for key in &reassigned {
            affected_bindings.remove(key);
        }
    }
    if !affected_bindings.is_empty() {
        let mut ref_rewriter = DefaultRefRewriter {
            affected: &affected_bindings,
        };
        module.visit_mut_with(&mut ref_rewriter);
    }

    // Phase 3: Remove helper declarations.
    if !helpers.is_empty() && !preserve_named_helpers {
        remove_helper_declarations(&mut module.body, &helpers);
    }
}

// ---------------------------------------------------------------------------
// Phase 1: Collect affected bindings
// ---------------------------------------------------------------------------

struct AffectedBindingCollector<'a> {
    local_helpers: &'a LocalHelperContext,
    affected: &'a mut HashSet<BindingKey>,
}

impl Visit for AffectedBindingCollector<'_> {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        let Pat::Ident(bi) = &decl.name else { return };
        let Some(init) = &decl.init else { return };

        // var _a = helper(arg)
        if is_helper_call(init, self.local_helpers) {
            self.affected.insert((bi.id.sym.clone(), bi.id.ctxt));
        }
    }
}

fn is_helper_call(expr: &Expr, local_helpers: &LocalHelperContext) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireDefault)
}

/// Collect standalone interop assignments that initialize a require-backed
/// binding in SWC AMD output. The assignment must be the binding's first use
/// outside its declaration; otherwise `.default` accesses before the wrapper
/// could refer to the unwrapped require value and must remain untouched.
fn collect_assignment_form_initializers(
    module: &Module,
    local_helpers: &LocalHelperContext,
    affected: &mut HashSet<BindingKey>,
) -> HashSet<usize> {
    let require_declarations = collect_top_level_require_declarations(module, local_helpers);
    let top_level_functions = collect_top_level_functions(module);
    let mut matched_indices = HashSet::new();

    for (index, item) in module.body.iter().enumerate() {
        let Some(binding) = assignment_form_initializer_binding(item, local_helpers) else {
            continue;
        };
        let Some(&declaration_index) = require_declarations.get(&binding) else {
            continue;
        };
        if declaration_index >= index {
            continue;
        }

        let used_before_initializer =
            module.body[..index]
                .iter()
                .enumerate()
                .any(|(prior_index, prior_item)| {
                    prior_index != declaration_index
                        && item_references_binding(prior_item, &binding)
                });
        if used_before_initializer {
            continue;
        }

        // A hoisted function that reads the binding can run before the
        // wrapper is installed if any earlier statement transfers control
        // into module code. Only accept prior items whose evaluation is
        // proven inert with respect to module functions: generated require
        // declarations, sibling interop initializers, generated
        // `Object.defineProperty`/descriptor export scaffolding, and calls
        // into local helper functions that neither touch the binding nor call
        // anything unrecognized.
        let pre_initializer_invocation_hazard =
            module.body[..index]
                .iter()
                .enumerate()
                .any(|(prior_index, prior_item)| {
                    prior_index != declaration_index
                        && item_can_invoke_pre_initializer_read(
                            prior_item,
                            &binding,
                            local_helpers,
                            &top_level_functions,
                        )
                });
        if pre_initializer_invocation_hazard {
            continue;
        }

        // The wrapper is a pure initialization only when the module contains
        // no other write to the binding. A later reassignment (in any form:
        // plain or compound assignment, update expression, loop head, or
        // destructuring target, including inside deferred function bodies)
        // means removing the wrapper while `.default` reads are rewritten
        // would change which value those reads observe.
        if binding_has_other_writes(module, &binding, index, declaration_index) {
            continue;
        }

        affected.insert(binding);
        matched_indices.insert(index);
    }

    matched_indices
}

fn collect_top_level_functions(module: &Module) -> HashMap<BindingKey, &Function> {
    let mut functions = HashMap::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) = item else {
            continue;
        };
        functions.insert(
            (fn_decl.ident.sym.clone(), fn_decl.ident.ctxt),
            fn_decl.function.as_ref(),
        );
    }
    functions
}

/// Whether evaluating this top-level item could call module code that reads
/// the interop binding before its wrapper assignment runs.
///
/// The scan whitelists exactly the calls SWC's generated module preamble
/// performs — `require("<literal>")`, recognized interop helpers, generated
/// export reflection, and locally declared helper functions whose bodies are
/// themselves proven inert. Everything else (unknown calls, `new`, tagged
/// templates, optional calls, class evaluation) fails closed.
/// `Object.getOwnPropertyDescriptor` is admitted only while proving one of
/// those local helpers; an arbitrary top-level reflection call can trigger a
/// proxy trap and remains blocked. The gate targets generated producer shapes:
/// it does not model ordinary property reads that could trigger previously
/// installed accessors, which no known transpiler preamble performs before its
/// interop initializers.
fn item_can_invoke_pre_initializer_read(
    item: &ModuleItem,
    binding: &BindingKey,
    local_helpers: &LocalHelperContext,
    top_level_functions: &HashMap<BindingKey, &Function>,
) -> bool {
    let visiting = RefCell::new(HashSet::new());
    let mut scanner = ImmediateInvocationScanner {
        binding,
        local_helpers,
        top_level_functions,
        visiting: &visiting,
        inside_local_helper: false,
        blocked: false,
    };
    item.visit_with(&mut scanner);
    scanner.blocked
}

struct ImmediateInvocationScanner<'a> {
    binding: &'a BindingKey,
    local_helpers: &'a LocalHelperContext,
    top_level_functions: &'a HashMap<BindingKey, &'a Function>,
    visiting: &'a RefCell<HashSet<BindingKey>>,
    inside_local_helper: bool,
    blocked: bool,
}

impl ImmediateInvocationScanner<'_> {
    fn call_is_permitted(&mut self, call: &CallExpr) -> bool {
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };
        if call_is_static_require(call, self.local_helpers) {
            return true;
        }
        if self
            .local_helpers
            .is_helper_callee(callee, TranspilerHelperKind::InteropRequireDefault)
            || self
                .local_helpers
                .is_helper_callee(callee, TranspilerHelperKind::InteropRequireWildcard)
        {
            return true;
        }
        if let Expr::Member(member) = callee.as_ref() {
            if let (Expr::Ident(object), MemberProp::Ident(prop)) =
                (member.obj.as_ref(), &member.prop)
            {
                if self
                    .local_helpers
                    .is_unresolved_or_unguarded_ident(object, "Object")
                    && (prop.sym.as_ref() == "defineProperty"
                        || (self.inside_local_helper
                            && prop.sym.as_ref() == "getOwnPropertyDescriptor"))
                {
                    return true;
                }
            }
        }
        if let Expr::Ident(callee_ident) = callee.as_ref() {
            let key = (callee_ident.sym.clone(), callee_ident.ctxt);
            return self.local_function_is_inert(&key);
        }
        false
    }

    fn local_function_is_inert(&self, key: &BindingKey) -> bool {
        let Some(function) = self.top_level_functions.get(key) else {
            return false;
        };
        if function_references_binding(function, self.binding) {
            return false;
        }
        if !self.visiting.borrow_mut().insert(key.clone()) {
            // Recursion between local helpers: fail closed rather than
            // assuming the cycle is inert.
            return false;
        }
        let mut inner = ImmediateInvocationScanner {
            binding: self.binding,
            local_helpers: self.local_helpers,
            top_level_functions: self.top_level_functions,
            visiting: self.visiting,
            inside_local_helper: true,
            blocked: false,
        };
        // A called function's parameters and body evaluate when it runs, so
        // scan them with the same immediate-evaluation rules; nested function
        // bodies stay deferred via the scanner's overrides.
        function.params.visit_with(&mut inner);
        function.body.visit_with(&mut inner);
        self.visiting.borrow_mut().remove(key);
        !inner.blocked
    }
}

impl Visit for ImmediateInvocationScanner<'_> {
    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_class(&mut self, _: &Class) {
        // Class evaluation can run computed keys and static blocks; the
        // generated preambles this gate models never contain classes.
        self.blocked = true;
    }

    fn visit_new_expr(&mut self, _: &NewExpr) {
        self.blocked = true;
    }

    fn visit_tagged_tpl(&mut self, _: &TaggedTpl) {
        self.blocked = true;
    }

    fn visit_opt_chain_expr(&mut self, chain: &OptChainExpr) {
        if self.blocked {
            return;
        }
        match &*chain.base {
            OptChainBase::Call(_) => self.blocked = true,
            OptChainBase::Member(_) => chain.visit_children_with(self),
        }
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if self.blocked {
            return;
        }
        if self.call_is_permitted(call) {
            for arg in &call.args {
                arg.visit_with(self);
            }
        } else {
            self.blocked = true;
        }
    }
}

fn function_references_binding(function: &Function, binding: &BindingKey) -> bool {
    let mut finder = BindingReferenceFinder {
        binding,
        found: false,
    };
    function.visit_with(&mut finder);
    finder.found
}

/// Whether any module code other than the matched initializer (and the
/// require declaration that created the binding) writes the binding.
fn binding_has_other_writes(
    module: &Module,
    binding: &BindingKey,
    initializer_index: usize,
    declaration_index: usize,
) -> bool {
    let mut finder = BindingWriteFinder {
        binding,
        found: false,
    };
    for (index, item) in module.body.iter().enumerate() {
        if index == initializer_index || index == declaration_index {
            continue;
        }
        item.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

struct BindingWriteFinder<'a> {
    binding: &'a BindingKey,
    found: bool,
}

impl BindingWriteFinder<'_> {
    fn check_for_head(&mut self, head: &ForHead) {
        match head {
            ForHead::Pat(pat) => {
                if pat_writes_binding(pat, self.binding) {
                    self.found = true;
                }
            }
            ForHead::VarDecl(decl) => {
                for declarator in &decl.decls {
                    if pat_writes_binding(&declarator.name, self.binding) {
                        self.found = true;
                    }
                }
            }
            ForHead::UsingDecl(_) => {}
        }
    }
}

impl Visit for BindingWriteFinder<'_> {
    fn visit_assign_expr(&mut self, node: &swc_core::ecma::ast::AssignExpr) {
        node.visit_children_with(self);
        match &node.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(target)) => {
                if (target.id.sym.clone(), target.id.ctxt) == *self.binding {
                    self.found = true;
                }
            }
            AssignTarget::Pat(pat) => {
                if assign_target_pat_writes_binding(pat, self.binding) {
                    self.found = true;
                }
            }
            AssignTarget::Simple(_) => {}
        }
    }

    fn visit_update_expr(&mut self, node: &UpdateExpr) {
        node.visit_children_with(self);
        if let Expr::Ident(ident) = node.arg.as_ref() {
            if (ident.sym.clone(), ident.ctxt) == *self.binding {
                self.found = true;
            }
        }
    }

    fn visit_for_in_stmt(&mut self, node: &ForInStmt) {
        self.check_for_head(&node.left);
        node.visit_children_with(self);
    }

    fn visit_for_of_stmt(&mut self, node: &ForOfStmt) {
        self.check_for_head(&node.left);
        node.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, node: &VarDeclarator) {
        node.visit_children_with(self);
        if node.init.is_some() && pat_writes_binding(&node.name, self.binding) {
            self.found = true;
        }
    }
}

fn assign_target_pat_writes_binding(
    pat: &swc_core::ecma::ast::AssignTargetPat,
    binding: &BindingKey,
) -> bool {
    use swc_core::ecma::ast::AssignTargetPat;
    match pat {
        AssignTargetPat::Array(array) => array
            .elems
            .iter()
            .flatten()
            .any(|element| pat_writes_binding(element, binding)),
        AssignTargetPat::Object(object) => object
            .props
            .iter()
            .any(|prop| object_pat_prop_writes_binding(prop, binding)),
        AssignTargetPat::Invalid(_) => false,
    }
}

fn pat_writes_binding(pat: &Pat, binding: &BindingKey) -> bool {
    match pat {
        Pat::Ident(ident) => (ident.id.sym.clone(), ident.id.ctxt) == *binding,
        Pat::Array(array) => array
            .elems
            .iter()
            .flatten()
            .any(|element| pat_writes_binding(element, binding)),
        Pat::Rest(rest) => pat_writes_binding(&rest.arg, binding),
        Pat::Object(object) => object
            .props
            .iter()
            .any(|prop| object_pat_prop_writes_binding(prop, binding)),
        Pat::Assign(assign) => pat_writes_binding(&assign.left, binding),
        Pat::Expr(expr) => {
            matches!(expr.as_ref(), Expr::Ident(ident) if (ident.sym.clone(), ident.ctxt) == *binding)
        }
        Pat::Invalid(_) => false,
    }
}

fn object_pat_prop_writes_binding(prop: &ObjectPatProp, binding: &BindingKey) -> bool {
    match prop {
        ObjectPatProp::KeyValue(key_value) => pat_writes_binding(&key_value.value, binding),
        ObjectPatProp::Assign(assign) => {
            (assign.key.id.sym.clone(), assign.key.id.ctxt) == *binding
        }
        ObjectPatProp::Rest(rest) => pat_writes_binding(&rest.arg, binding),
    }
}

fn collect_top_level_require_declarations(
    module: &Module,
    local_helpers: &LocalHelperContext,
) -> HashMap<BindingKey, usize> {
    let mut declarations = HashMap::new();

    for (index, item) in module.body.iter().enumerate() {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
            continue;
        };
        // Assignment-form interop output comes from mutable AMD factory
        // parameters. Consuming an assignment to authored `const` would erase
        // its mandatory TypeError; `let` has no AMD provenance either.
        if var_decl.kind != VarDeclKind::Var {
            continue;
        }
        // AMD extraction emits one declaration per dependency. Requiring that
        // exact shape also avoids overlooking another use in the same item.
        let [declarator] = var_decl.decls.as_slice() else {
            continue;
        };
        let Pat::Ident(binding) = &declarator.name else {
            continue;
        };
        let Some(init) = &declarator.init else {
            continue;
        };
        if is_static_require_call(init, local_helpers) {
            declarations.insert((binding.id.sym.clone(), binding.id.ctxt), index);
        }
    }

    declarations
}

fn is_static_require_call(expr: &Expr, local_helpers: &LocalHelperContext) -> bool {
    let Expr::Call(call) = expr else { return false };
    call_is_static_require(call, local_helpers)
}

fn call_is_static_require(call: &CallExpr, local_helpers: &LocalHelperContext) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(require) = callee.as_ref() else {
        return false;
    };
    if !local_helpers.is_unresolved_or_unguarded_ident(require, "require") {
        return false;
    }
    let [arg] = call.args.as_slice() else {
        return false;
    };
    arg.spread.is_none() && matches!(arg.expr.as_ref(), Expr::Lit(Lit::Str(_)))
}

fn assignment_form_initializer_binding(
    item: &ModuleItem,
    local_helpers: &LocalHelperContext,
) -> Option<BindingKey> {
    let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
        return None;
    };
    let Expr::Assign(assign) = expr_stmt.expr.as_ref() else {
        return None;
    };
    assignment_form_binding(assign, local_helpers)
}

fn assignment_form_binding(
    assign: &AssignExpr,
    local_helpers: &LocalHelperContext,
) -> Option<BindingKey> {
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assign.left else {
        return None;
    };
    let Expr::Call(call) = assign.right.as_ref() else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireDefault) {
        return None;
    }
    let [arg] = call.args.as_slice() else {
        return None;
    };
    if arg.spread.is_some() {
        return None;
    }
    let Expr::Ident(argument) = arg.expr.as_ref() else {
        return None;
    };
    let target_key = (target.id.sym.clone(), target.id.ctxt);
    (target_key == (argument.sym.clone(), argument.ctxt)).then_some(target_key)
}

fn item_references_binding(item: &ModuleItem, binding: &BindingKey) -> bool {
    let mut finder = BindingReferenceFinder {
        binding,
        found: false,
    };
    item.visit_with(&mut finder);
    finder.found
}

struct BindingReferenceFinder<'a> {
    binding: &'a BindingKey,
    found: bool,
}

impl Visit for BindingReferenceFinder<'_> {
    fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
        if (ident.sym.clone(), ident.ctxt) == *self.binding {
            self.found = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 2a: Unwrap helper calls
// ---------------------------------------------------------------------------

struct CallUnwrapper<'a> {
    local_helpers: &'a LocalHelperContext,
    preserved_assignment_form: bool,
}

impl VisitMut for CallUnwrapper<'_> {
    fn visit_mut_assign_expr(&mut self, assign: &mut AssignExpr) {
        // Accepted top-level assignment initializers were removed before this
        // pass. Any exact self-wrapper assignment still present was rejected
        // by the safety proof (or was not an unconditional top-level item), so
        // keep both the call and the helper that implements its semantics.
        if assignment_form_binding(assign, self.local_helpers).is_some() {
            self.preserved_assignment_form = true;
            return;
        }
        assign.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        // helper(arg).default → arg
        if let Expr::Member(member) = expr {
            if is_default_prop(&member.prop) {
                if let Expr::Call(call) = member.obj.as_ref() {
                    if let Some(arg) = extract_helper_call_arg(call, self.local_helpers) {
                        *expr = arg;
                        return;
                    }
                }
            }
        }

        // helper(arg) → arg
        if let Expr::Call(call) = expr {
            if let Some(arg) = extract_helper_call_arg(call, self.local_helpers) {
                *expr = arg;
            }
        }
    }
}

fn extract_helper_call_arg(
    call: &swc_core::ecma::ast::CallExpr,
    local_helpers: &LocalHelperContext,
) -> Option<Expr> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireDefault) {
        return None;
    }
    if call.args.len() != 1 {
        return None;
    }
    Some(*call.args[0].expr.clone())
}

// ---------------------------------------------------------------------------
// Phase 2b (pre): Check for reassignment of affected bindings
// ---------------------------------------------------------------------------

struct ReassignmentChecker<'a> {
    candidates: &'a HashSet<BindingKey>,
    reassigned: &'a mut HashSet<BindingKey>,
}

impl Visit for ReassignmentChecker<'_> {
    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Ident(id)) = &assign.left {
            let key = (id.id.sym.clone(), id.id.ctxt);
            if self.candidates.contains(&key) {
                self.reassigned.insert(key);
            }
        }
        assign.visit_children_with(self);
    }
}

// ---------------------------------------------------------------------------
// Phase 2b: Rewrite .default references on affected bindings
// ---------------------------------------------------------------------------

struct DefaultRefRewriter<'a> {
    affected: &'a HashSet<BindingKey>,
}

impl VisitMut for DefaultRefRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        // x.default → x  (or x["default"] → x, already normalized by UnBracketNotation)
        if let Expr::Member(member) = expr {
            if is_default_prop(&member.prop) {
                if let Expr::Ident(obj) = member.obj.as_ref() {
                    if self.affected.contains(&(obj.sym.clone(), obj.ctxt)) {
                        *expr = Expr::Ident(obj.clone());
                        return;
                    }
                }
            }
        }

        // Match before descending so a chain such as `x.default.default`
        // loses exactly the innermost wrapper layer. A bottom-up visitor would
        // turn the inner member into `x`, then immediately match the outer
        // member too and silently erase an authored `.default` access.
        expr.visit_mut_children_with(self);
    }
}

// ---------------------------------------------------------------------------
// Inline IIFE interop detection and unwrapping
// ---------------------------------------------------------------------------

/// Detect and unwrap inline interop IIFEs:
/// ```js
/// const x = ((e) => {
///     if (e && e.__esModule) { return e; }
///     return { default: e };
/// })(require("./module.js"));
/// ```
/// → `const x = require("./module.js")`
fn unwrap_inline_interop_iifes(module: &mut Module, affected: &mut HashSet<BindingKey>) {
    for item in &mut module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
            continue;
        };
        for declarator in &mut var_decl.decls {
            let Pat::Ident(binding) = &declarator.name else {
                continue;
            };
            let Some(init) = &declarator.init else {
                continue;
            };
            let Expr::Call(call) = init.as_ref() else {
                continue;
            };
            let Some((kind, inner_arg)) = classify_inline_helper_call(call) else {
                continue;
            };
            // Only strip `.default` for the default interop, not wildcard.
            match kind {
                TranspilerHelperKind::InteropRequireDefault => {
                    let key = (binding.id.sym.clone(), binding.id.ctxt);
                    affected.insert(key);
                }
                TranspilerHelperKind::InteropRequireWildcard => {}
                // Other helper IIFEs are handled by their own rules.
                _ => continue,
            }
            let inner_arg = Box::new(inner_arg.clone());
            declarator.init = Some(inner_arg);
        }
    }
}

fn is_default_prop(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(id) => id.sym.as_ref() == "default",
        MemberProp::Computed(c) => {
            matches!(c.expr.as_ref(), Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some("default"))
        }
        _ => false,
    }
}
