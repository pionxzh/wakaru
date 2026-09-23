use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, ArrowExpr, ArrowFunctionBody, CallExpr, Callee, Constructor, Decl, DefaultDecl, Expr,
    ExprOrSpread, Function, FunctionBody, Ident, MemberProp, Module, ModuleDecl, ModuleItem,
    ParamOrTsParamProp, Pat, Stmt, VarDeclKind,
};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::{binding_id, BindingId};
use crate::collections::{HashMap, HashSet};

use super::arg_rest::find_rest_array_copy_proof;
use super::eval_utils::has_dynamic_scope_construct;
use super::RewriteLevel;

/// Flattens array-literal `.concat(...)` calls into one array literal.
///
/// At Minimal and Standard, every argument must be an array literal. Aggressive
/// also treats unknown arguments as arrays, preserving the old generated-code
/// heuristic for Babel loose / `iterableIsArray` output under the
/// `concat_arguments_are_arrays` assumption.
///
/// Handles:
/// - `[a].concat([b, c])` → `[a, b, c]`
/// - `[a].concat([b], [c])` → `[a, b, c]`
/// - Aggressive: `[a].concat(items)` → `[a, ...items]`
///
/// Only transforms when the receiver is an **array literal** — variable
/// receivers like `arr.concat(other)` are left as-is since `concat` may be
/// overridden or the receiver may not be a plain array.
///
/// `concat` only spreads Array / `@@isConcatSpreadable`; spread iterates any
/// iterable and throws on non-iterables. Flattening nested array literals is
/// concat-faithful for ordinary arrays; patched `Array.prototype.concat` or
/// `Symbol.isConcatSpreadable` can still differ. This remains a generated-code
/// heuristic, with unknown values restricted to Aggressive.
pub struct UnArrayConcatSpread {
    level: RewriteLevel,
}

impl UnArrayConcatSpread {
    pub fn new() -> Self {
        Self::new_with_level(RewriteLevel::Standard)
    }

    pub fn new_with_level(level: RewriteLevel) -> Self {
        Self { level }
    }
}

impl Default for UnArrayConcatSpread {
    fn default() -> Self {
        Self::new()
    }
}

impl VisitMut for UnArrayConcatSpread {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };

        if let Some(new_arr) = try_simplify_array_concat(call, self.level, None) {
            *expr = Expr::Array(new_arr);
        }
    }
}

/// A proof-aware second pass for concat operands whose Array identity is
/// proven by resolver-identified bindings.
///
/// The early concat pass cannot treat an identifier as an Array. This pass runs
/// after parameter-shape cleanup and immediately before class recovery, while
/// the exact rest-copy shape is still available. It proves these bindings in
/// the function or module body that declares them:
///
/// - rest parameters and canonical Babel/TypeScript `arguments`-copy arrays;
/// - `var`/`let`/`const` bindings initialized with a hole-free array literal;
/// - functions whose whole body returns a hole-free array literal, so every
///   call yields a fresh Array no other code can reach.
///
/// A value binding is accepted only when every use after initialization is an
/// operand of an intrinsic concat: an argument of an array-literal or proven
/// receiver, or the receiver of its own `.concat(...)`. A function binding is
/// accepted only when every use is a direct call. Reassignment, escape,
/// mutation (including `Symbol.isConcatSpreadable`), and reads before a `var`
/// initializer all fail closed.
pub struct UnArrayConcatSpreadRest {
    unresolved_mark: Mark,
    level: RewriteLevel,
}

impl UnArrayConcatSpreadRest {
    pub fn new(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
        }
    }

    fn recover_in_function_body(
        &self,
        body: &mut FunctionBody,
        params: &[&Pat],
        fixed_param_count: Option<usize>,
    ) {
        if has_dynamic_scope_construct(body) {
            return;
        }

        let mut param_bindings = HashSet::default();
        for param in params {
            param_bindings.extend(find_pat_ids::<_, BindingId>(*param));
        }

        let mut candidates = params
            .iter()
            .filter_map(|param| rest_binding(param))
            .map(|binding| Candidate {
                binding,
                kind: ProofKind::Value,
                start: 0,
                ready: 0,
                init_refs: None,
                own_refs: 0,
                hoisted_var: false,
            })
            .collect::<Vec<_>>();
        if let Some(copy) = fixed_param_count
            .and_then(|count| find_rest_array_copy_proof(body, count, self.unresolved_mark))
        {
            candidates.push(Candidate {
                binding: binding_id(&copy.binding),
                kind: ProofKind::Value,
                start: copy.start_stmt,
                ready: copy.ready_stmt,
                init_refs: None,
                own_refs: 0,
                hoisted_var: true,
            });
        }
        let copied = candidates
            .iter()
            .map(|candidate| candidate.binding.clone())
            .collect::<HashSet<_>>();
        candidates.extend(
            collect_declared_candidates(&body.stmts, &param_bindings)
                .into_iter()
                .filter(|candidate| !copied.contains(&candidate.binding)),
        );

        recover_proven_array_concat(&mut body.stmts, candidates);
    }
}

impl VisitMut for UnArrayConcatSpreadRest {
    fn visit_mut_module(&mut self, module: &mut Module) {
        module.visit_mut_children_with(self);
        if self.level < RewriteLevel::Standard || has_dynamic_scope_construct(module) {
            return;
        }

        let candidates = collect_declared_candidates(&module.body, &HashSet::default());
        recover_proven_array_concat(&mut module.body, candidates);
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        function.visit_mut_children_with(self);
        if self.level < RewriteLevel::Standard {
            return;
        }

        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let fixed_param_count = function.params.len();
        let Some(body) = &mut function.body else {
            return;
        };
        self.recover_in_function_body(body, &params, Some(fixed_param_count));
    }

    fn visit_mut_constructor(&mut self, constructor: &mut Constructor) {
        constructor.visit_mut_children_with(self);
        if self.level < RewriteLevel::Standard {
            return;
        }

        // A TypeScript parameter property is also a parameter binding; leave
        // those constructors to the class rules instead of modeling it here.
        let Some(params) = constructor
            .params
            .iter()
            .map(|param| match param {
                ParamOrTsParamProp::Param(param) => Some(&param.pat),
                ParamOrTsParamProp::TsParamProp(_) => None,
            })
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        let fixed_param_count = constructor.params.len();
        let Some(body) = &mut constructor.body else {
            return;
        };
        self.recover_in_function_body(body, &params, Some(fixed_param_count));
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        arrow.visit_mut_children_with(self);
        if self.level < RewriteLevel::Standard {
            return;
        }

        let params = arrow.params.iter().collect::<Vec<_>>();
        let ArrowFunctionBody::FunctionBody(body) = arrow.body.as_mut() else {
            return;
        };
        // Arrows have no own `arguments`, so there is no copy loop to prove.
        self.recover_in_function_body(body, &params, None);
    }
}

fn rest_binding(pat: &Pat) -> Option<BindingId> {
    let Pat::Rest(rest) = pat else {
        return None;
    };
    let Pat::Ident(binding) = rest.arg.as_ref() else {
        return None;
    };
    Some(binding_id(&binding.id))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProofKind {
    /// The binding holds an Array at every proven use.
    Value,
    /// Every call of the binding returns a fresh, hole-free Array literal.
    Factory,
}

struct Candidate {
    binding: BindingId,
    kind: ProofKind,
    /// References in body items before this index fail the proof.
    start: usize,
    /// References from this index on must be proof positions.
    ready: usize,
    /// Exact reference count required in `start..ready`; `None` ignores that
    /// range (the rest-copy loop writes the array there).
    init_refs: Option<usize>,
    /// References after `ready` allowed outside proof positions: a function
    /// declaration's own name.
    own_refs: usize,
    /// A `var` reads as `undefined` before its initializer. A hoisted function
    /// declaration can run first, so it must not reference the binding.
    hoisted_var: bool,
}

/// Body items the proof walks: function statements or module items.
trait BodyItem {
    fn as_stmt(&self) -> Option<&Stmt>;
    fn is_function_declaration(&self) -> bool;
}

impl BodyItem for Stmt {
    fn as_stmt(&self) -> Option<&Stmt> {
        Some(self)
    }

    fn is_function_declaration(&self) -> bool {
        matches!(self, Stmt::Decl(Decl::Fn(_)))
    }
}

impl BodyItem for ModuleItem {
    fn as_stmt(&self) -> Option<&Stmt> {
        match self {
            ModuleItem::Stmt(stmt) => Some(stmt),
            ModuleItem::ModuleDecl(_) => None,
        }
    }

    fn is_function_declaration(&self) -> bool {
        match self {
            ModuleItem::Stmt(stmt) => stmt.is_function_declaration(),
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                matches!(export.decl, Decl::Fn(_))
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(export)) => {
                matches!(export.decl, DefaultDecl::Fn(_))
            }
            ModuleItem::ModuleDecl(_) => false,
        }
    }
}

/// Collect array-literal bindings and array-returning functions declared
/// directly in this body. Exported declarations are not candidates: another
/// module could observe the array.
fn collect_declared_candidates<T: BodyItem>(
    items: &[T],
    excluded: &HashSet<BindingId>,
) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match item.as_stmt() {
            Some(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    let Pat::Ident(name) = &decl.name else {
                        continue;
                    };
                    let binding = binding_id(&name.id);
                    if excluded.contains(&binding) {
                        continue;
                    }
                    let Some(init) = decl.init.as_deref() else {
                        continue;
                    };
                    let kind = if is_dense_array_literal(init) {
                        ProofKind::Value
                    } else if returns_fresh_array(init) {
                        ProofKind::Factory
                    } else {
                        continue;
                    };
                    candidates.push(Candidate {
                        binding,
                        kind,
                        start: index,
                        ready: index + 1,
                        init_refs: Some(1),
                        own_refs: 0,
                        // Calling an uninitialized factory throws the same
                        // TypeError in both forms.
                        hoisted_var: kind == ProofKind::Value && var.kind == VarDeclKind::Var,
                    });
                }
            }
            Some(Stmt::Decl(Decl::Fn(function)))
                if function_returns_fresh_array(&function.function) =>
            {
                let binding = binding_id(&function.ident);
                if excluded.contains(&binding) {
                    continue;
                }
                candidates.push(Candidate {
                    binding,
                    kind: ProofKind::Factory,
                    start: 0,
                    ready: 0,
                    init_refs: None,
                    own_refs: 1,
                    hoisted_var: false,
                });
            }
            _ => {}
        }
    }
    candidates
}

/// Concat keeps holes; spread reads them as `undefined`.
fn is_dense_array_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::Array(array) if array.elems.iter().all(Option::is_some))
}

fn returns_fresh_array(expr: &Expr) -> bool {
    match expr {
        Expr::Arrow(arrow) => {
            !arrow.is_async
                && !arrow.is_generator
                && match arrow.body.as_ref() {
                    ArrowFunctionBody::Expr(body) => is_dense_array_literal(body),
                    ArrowFunctionBody::FunctionBody(body) => body_returns_fresh_array(body),
                }
        }
        Expr::Fn(function) => function_returns_fresh_array(&function.function),
        _ => false,
    }
}

fn function_returns_fresh_array(function: &Function) -> bool {
    !function.is_async
        && !function.is_generator
        && function.body.as_ref().is_some_and(body_returns_fresh_array)
}

fn body_returns_fresh_array(body: &FunctionBody) -> bool {
    matches!(
        body.stmts.as_slice(),
        [Stmt::Return(ret)] if ret.arg.as_deref().is_some_and(is_dense_array_literal)
    )
}

fn recover_proven_array_concat<T>(items: &mut [T], candidates: Vec<Candidate>)
where
    T: BodyItem,
    for<'a> T: VisitWith<ProofUseScanner<'a>> + VisitMutWith<ProvenConcatRewriter<'a>>,
{
    if candidates.is_empty() {
        return;
    }

    // A binding declared twice in one body has no single initialization point.
    let mut by_binding: HashMap<BindingId, Candidate> = HashMap::default();
    let mut duplicated = HashSet::default();
    for candidate in candidates {
        if by_binding.contains_key(&candidate.binding) {
            duplicated.insert(candidate.binding.clone());
        } else {
            by_binding.insert(candidate.binding.clone(), candidate);
        }
    }
    for binding in &duplicated {
        by_binding.remove(binding);
    }
    if by_binding.is_empty() {
        return;
    }

    let mut scanner = ProofUseScanner {
        candidates: &by_binding,
        uses: HashMap::default(),
        item: 0,
        in_function_declaration: false,
    };
    for (index, item) in items.iter().enumerate() {
        scanner.item = index;
        scanner.in_function_declaration = item.is_function_declaration();
        item.visit_with(&mut scanner);
    }
    let uses = scanner.uses;

    let mut proven = by_binding
        .values()
        .filter(|candidate| {
            let Some(uses) = uses.get(&candidate.binding) else {
                return candidate.init_refs.is_none();
            };
            uses.before == 0
                && candidate.init_refs.is_none_or(|count| uses.init == count)
                && uses.after - uses.after_positions == candidate.own_refs
                && !uses.hoisted_read
        })
        .map(|candidate| candidate.binding.clone())
        .collect::<HashSet<_>>();

    // An argument of `receiver.concat(...)` is only an intrinsic-concat
    // operand while the receiver itself stays proven.
    loop {
        let failed = proven
            .iter()
            .filter(|binding| {
                uses.get(*binding).is_some_and(|uses| {
                    uses.receivers
                        .iter()
                        .any(|receiver| !proven.contains(receiver))
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if failed.is_empty() {
            break;
        }
        for binding in failed {
            proven.remove(&binding);
        }
    }
    if proven.is_empty() {
        return;
    }

    let mut arrays = ProvenArrays::default();
    for binding in proven {
        match by_binding[&binding].kind {
            ProofKind::Value => arrays.values.insert(binding),
            ProofKind::Factory => arrays.factories.insert(binding),
        };
    }
    let mut rewriter = ProvenConcatRewriter { arrays: &arrays };
    for item in items.iter_mut() {
        item.visit_mut_with(&mut rewriter);
    }
}

#[derive(Default)]
struct CandidateUses {
    before: usize,
    init: usize,
    after: usize,
    after_positions: usize,
    hoisted_read: bool,
    /// Candidate receivers whose `.concat(...)` takes this binding.
    receivers: HashSet<BindingId>,
}

struct ProofUseScanner<'a> {
    candidates: &'a HashMap<BindingId, Candidate>,
    uses: HashMap<BindingId, CandidateUses>,
    item: usize,
    in_function_declaration: bool,
}

impl ProofUseScanner<'_> {
    fn candidate_of_kind(&self, expr: &Expr, kind: ProofKind) -> Option<BindingId> {
        let Expr::Ident(ident) = expr else {
            return None;
        };
        let binding = binding_id(ident);
        self.candidates
            .get(&binding)
            .is_some_and(|candidate| candidate.kind == kind)
            .then_some(binding)
    }

    /// `Some` when `expr` is an Array literal, a candidate array binding, or an
    /// intrinsic concat result built on one; the inner value names the
    /// candidate the Array proof depends on.
    fn array_receiver_root(&self, expr: &Expr) -> Option<Option<BindingId>> {
        match expr {
            Expr::Array(_) => Some(None),
            Expr::Ident(_) => self.candidate_of_kind(expr, ProofKind::Value).map(Some),
            Expr::Call(call) => self.array_receiver_root(concat_receiver(call)?),
            _ => None,
        }
    }

    fn record_position(&mut self, binding: &BindingId, receiver: Option<&BindingId>) {
        if self.item < self.candidates[binding].ready {
            return;
        }
        let uses = self.uses.entry(binding.clone()).or_default();
        uses.after_positions += 1;
        if let Some(receiver) = receiver.filter(|receiver| *receiver != binding) {
            uses.receivers.insert(receiver.clone());
        }
    }
}

impl Visit for ProofUseScanner<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        let binding = binding_id(ident);
        let Some(candidate) = self.candidates.get(&binding) else {
            return;
        };
        let uses = self.uses.entry(binding).or_default();
        if self.item < candidate.start {
            uses.before += 1;
        } else if self.item < candidate.ready {
            uses.init += 1;
        } else {
            uses.after += 1;
            if self.in_function_declaration && candidate.hoisted_var {
                uses.hoisted_read = true;
            }
        }
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(receiver) = concat_receiver(call) {
            // Only an Array receiver makes `.concat` the intrinsic method.
            if let Some(receiver_binding) = self.array_receiver_root(receiver) {
                if let Some(binding) = self.candidate_of_kind(receiver, ProofKind::Value) {
                    self.record_position(&binding, None);
                }
                for arg in &call.args {
                    if arg.spread.is_some() {
                        continue;
                    }
                    if let Some(binding) = self.candidate_of_kind(&arg.expr, ProofKind::Value) {
                        self.record_position(&binding, receiver_binding.as_ref());
                    }
                }
            }
        }
        if let Callee::Expr(callee) = &call.callee {
            if let Some(binding) = self.candidate_of_kind(callee, ProofKind::Factory) {
                self.record_position(&binding, None);
            }
        }
        call.visit_children_with(self);
    }
}

#[derive(Default)]
struct ProvenArrays {
    values: HashSet<BindingId>,
    factories: HashSet<BindingId>,
}

impl ProvenArrays {
    fn proves(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(ident) => self.values.contains(&binding_id(ident)),
            Expr::Call(call) => matches!(
                &call.callee,
                Callee::Expr(callee)
                    if matches!(callee.as_ref(), Expr::Ident(ident)
                        if self.factories.contains(&binding_id(ident)))
            ),
            _ => false,
        }
    }
}

struct ProvenConcatRewriter<'a> {
    arrays: &'a ProvenArrays,
}

impl VisitMut for ProvenConcatRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        if let Some(new_arr) =
            try_simplify_array_concat(call, RewriteLevel::Standard, Some(self.arrays))
        {
            *expr = Expr::Array(new_arr);
        }
    }
}

/// Try to convert `[elems].concat(args...)` into a single array literal.
fn try_simplify_array_concat(
    call: &CallExpr,
    level: RewriteLevel,
    proven: Option<&ProvenArrays>,
) -> Option<ArrayLit> {
    let proves = |expr: &Expr| proven.is_some_and(|arrays| arrays.proves(expr));
    let receiver = concat_receiver(call)?;

    if call.args.is_empty() {
        return None;
    }

    // `[].concat(...arr)` flattens sub-arrays via concat's built-in behavior,
    // but `[...arr]` does not.
    if call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }

    let mut elems: Vec<Option<ExprOrSpread>> = match receiver {
        Expr::Array(receiver_arr) => receiver_arr.elems.clone(),
        Expr::Ident(_) if proves(receiver) => vec![Some(spread_elem(receiver))],
        _ => return None,
    };

    for arg in &call.args {
        match arg.expr.as_ref() {
            Expr::Array(arr) => elems.extend(arr.elems.iter().cloned()),
            expr if proves(expr) || level >= RewriteLevel::Aggressive => {
                elems.push(Some(spread_elem(expr)));
            }
            _ => return None,
        }
    }

    Some(ArrayLit {
        span: DUMMY_SP,
        elems,
    })
}

fn spread_elem(expr: &Expr) -> ExprOrSpread {
    ExprOrSpread {
        spread: Some(DUMMY_SP),
        expr: Box::new(expr.clone()),
    }
}

fn concat_receiver(call: &CallExpr) -> Option<&Expr> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    let MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    if prop.sym != "concat" {
        return None;
    }
    Some(member.obj.as_ref())
}
