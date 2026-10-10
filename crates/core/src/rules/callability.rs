use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp,
    BindingIdent, CallExpr, Callee, Class, Decl, DefaultDecl, ExportSpecifier, Expr, FnDecl,
    Function, MemberExpr, ModuleDecl, ModuleExportName, ModuleItem, NewExpr, Pat, ReturnStmt,
    SimpleAssignTarget, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::utils::paren::strip_parens;

use super::helper_matcher::{
    binding_key, expr_binding_key, member_prop_name, static_member_prop_name, BindingKey,
};

/// Bindings whose current uses still require an ordinary function's
/// `[[Call]]`. Function-to-class rules must not recover these bindings until
/// the requiring call has been consumed by another proven class recovery.
///
/// Only `.call` / `.apply` (lowered `super` calls) are recorded. A plain
/// `F()` call does not block recovery: once a rule has proven a transpiler
/// class shape, requiring `new` is an accepted function-to-class change
/// (`docs/rewrite-assumptions.md`, `native_class_inheritance`). Rules that
/// rebuild a class shape themselves must find their own class evidence.
///
/// The same walk records constructor members the scope constructs, which
/// class recovery must not turn into methods (`member_constructed`).
pub(crate) struct CallabilityIndex {
    required: HashSet<BindingKey>,
    constructed_members: HashSet<MemberKey>,
}

/// A member of a constructor binding: the binding, whether the member is
/// static (`C.k`) or on the prototype (`C.prototype.k`), and its name.
type MemberKey = (BindingKey, bool, Atom);

impl CallabilityIndex {
    pub(crate) fn collect_stmts(stmts: &[Stmt]) -> Self {
        collect(stmts, &HashSet::default())
    }

    /// Same collection as a module walk, but `roots` are required before alias
    /// propagation. Cross-file pins use this so an exported `var Foo = IIFE`
    /// also keeps the constructor that IIFE returns.
    pub(crate) fn collect_module_items_with_roots(
        items: &[ModuleItem],
        roots: &HashSet<BindingKey>,
    ) -> Self {
        collect(items, roots)
    }

    pub(crate) fn collect_stmts_with_roots(stmts: &[Stmt], roots: &HashSet<BindingKey>) -> Self {
        collect(stmts, roots)
    }

    pub(crate) fn requires_call(&self, binding: &BindingKey) -> bool {
        self.required.contains(binding)
    }

    /// Whether the scope constructs this member of `owner` or reads its
    /// `prototype`: `new C.k`, `C.k.prototype`, `x instanceof C.k`,
    /// `extends C.k`, and the `C.prototype.k` forms, through the same aliases
    /// as `requires_call`. A method has no `[[Construct]]` or `prototype`, so
    /// class recovery keeps such a member a function. Construction the scope
    /// cannot see is not covered.
    pub(crate) fn member_constructed(
        &self,
        owner: &BindingKey,
        is_static: bool,
        name: &str,
    ) -> bool {
        self.constructed_members
            .contains(&(owner.clone(), is_static, Atom::from(name)))
    }
}

/// One `.call` / alias fact from a module walk.
///
/// `guards` are converted class owners that drop this fact. That is the result
/// alias, the IIFE parameter alias, and a constructor `super` call — not every
/// call inside the initializer. Method bodies keep their `.call`.
pub(crate) struct GuardedCallEffect {
    pub guards: Vec<BindingKey>,
    pub kind: GuardedCallEffectKind,
}

pub(crate) enum GuardedCallEffectKind {
    Required(BindingKey),
    /// Requiring `target` also requires `source`.
    Alias {
        target: BindingKey,
        source: BindingKey,
    },
    /// `||` / comma-tail value edge. Closed only from a live `.call` / `.apply`
    /// seed (and the ordinary alias closure of that seed), never from export
    /// roots. Guards stay empty: the edge is structural and must outlive the
    /// IIFE a class recovery deletes.
    ValueAlias {
        target: BindingKey,
        source: BindingKey,
    },
}

fn collect<N>(node: &N, roots: &HashSet<BindingKey>) -> CallabilityIndex
where
    N: VisitWith<CallabilityCollector> + ?Sized,
{
    let mut collector = CallabilityCollector::default();
    node.visit_with(&mut collector);
    // `.call` / `.apply` seeds walk ordinary aliases and value aliases.
    // Export roots walk ordinary aliases only, so a decorator `||` tail cannot
    // pin a constructor that no same-file call actually reaches.
    let ordinary = alias_map(&collector.aliases);
    let value = alias_map(&collector.value_aliases);
    let mut required = collector.required;
    close_required(&mut required, &ordinary, &value, true);
    let mut from_roots = roots.clone();
    close_required(&mut from_roots, &ordinary, &value, false);
    required.extend(from_roots);
    let constructed_members =
        close_constructed_members(collector.constructed_members, &ordinary, &value);

    CallabilityIndex {
        required,
        constructed_members,
    }
}

/// `target` evaluates to `source`, so constructing `target.k` constructs
/// `source.k`. Both ordinary and value aliases count: a missed edge turns a
/// constructible member into a method, an extra one only keeps a function.
fn close_constructed_members(
    mut members: HashSet<MemberKey>,
    ordinary: &HashMap<BindingKey, Vec<BindingKey>>,
    value: &HashMap<BindingKey, Vec<BindingKey>>,
) -> HashSet<MemberKey> {
    let mut pending: Vec<MemberKey> = members.iter().cloned().collect();
    while let Some((target, is_static, name)) = pending.pop() {
        for sources in [ordinary.get(&target), value.get(&target)]
            .into_iter()
            .flatten()
        {
            for source in sources {
                let member = (source.clone(), is_static, name.clone());
                if members.insert(member.clone()) {
                    pending.push(member);
                }
            }
        }
    }
    members
}

/// `C.k` → `(C, static, k)`; `C.prototype.k` → `(C, prototype, k)`.
fn member_key(expr: &Expr) -> Option<MemberKey> {
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    let name = static_member_prop_name(&member.prop)?;
    if name == "prototype" {
        return None;
    }
    match strip_parens(&member.obj) {
        Expr::Ident(owner) => Some((binding_key(owner), true, Atom::from(name))),
        Expr::Member(inner) if member_prop_name(&inner.prop, "prototype") => {
            let Expr::Ident(owner) = strip_parens(&inner.obj) else {
                return None;
            };
            Some((binding_key(owner), false, Atom::from(name)))
        }
        _ => None,
    }
}

#[derive(Default)]
struct CallabilityCollector {
    required: HashSet<BindingKey>,
    /// Members constructed or whose `prototype` is read; see
    /// [`CallabilityIndex::member_constructed`].
    constructed_members: HashSet<MemberKey>,
    /// `target` evaluates to `source`: requiring `target.[[Call]]` therefore
    /// requires `source.[[Call]]` too. Export roots may follow these edges.
    aliases: Vec<(BindingKey, BindingKey)>,
    /// Comma-tail and `||` edges. Live `.call` / `.apply` seeds may follow
    /// them; export roots may not.
    value_aliases: Vec<(BindingKey, BindingKey)>,
    /// Set only by [`module_guarded_call_effects`]. The index path leaves this
    /// empty so propagation stays on `required` and `aliases`.
    effects: Vec<GuardedCallEffect>,
    record_effects: bool,
    blankable: HashSet<BindingKey>,
    guard_stack: Vec<BindingKey>,
    /// Innermost blankable IIFE whose initializer is being walked.
    iife_scopes: Vec<IifeConsumeScope>,
    function_depth: usize,
    pending_constructor: bool,
    /// `true` while the current function is the IIFE's constructor.
    in_constructor: Vec<bool>,
}

struct IifeConsumeScope {
    owner: BindingKey,
    super_param: Option<BindingKey>,
    saw_constructor: bool,
    /// `function_depth` before entering this IIFE's callee.
    base_depth: usize,
}

impl CallabilityCollector {
    fn note_required(&mut self, binding: BindingKey) {
        if self.record_effects {
            self.effects.push(GuardedCallEffect {
                guards: self.guard_stack.clone(),
                kind: GuardedCallEffectKind::Required(binding.clone()),
            });
        }
        self.required.insert(binding);
    }

    fn note_alias(&mut self, target: BindingKey, source: BindingKey) {
        if self.record_effects {
            self.effects.push(GuardedCallEffect {
                guards: self.guard_stack.clone(),
                kind: GuardedCallEffectKind::Alias {
                    target: target.clone(),
                    source: source.clone(),
                },
            });
        }
        self.aliases.push((target, source));
    }

    /// Structural value edge. Empty guards: blanking a converted IIFE must not
    /// drop the path from a `.call` that still exists outside that IIFE.
    fn note_value_alias(&mut self, target: BindingKey, source: BindingKey) {
        if self.record_effects {
            self.effects.push(GuardedCallEffect {
                guards: Vec::new(),
                kind: GuardedCallEffectKind::ValueAlias {
                    target: target.clone(),
                    source: source.clone(),
                },
            });
        }
        self.value_aliases.push((target, source));
    }

    fn record_iife_param_aliases(&mut self, call: &CallExpr) {
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let inner = strip_parens(callee);
        let params: Vec<&Pat> = match inner {
            Expr::Fn(function) => function
                .function
                .params
                .iter()
                .map(|param| &param.pat)
                .collect(),
            Expr::Arrow(arrow) => arrow.params.iter().collect(),
            _ => return,
        };

        let mut spreads_seen = 0;
        for (argument_index, argument) in call.args.iter().enumerate() {
            if argument.spread.is_some() {
                spreads_seen += 1;
                continue;
            }
            let Some(argument_key) = expr_binding_key(strip_parens(&argument.expr)) else {
                continue;
            };

            if spreads_seen == 0 {
                if let Some(parameter_key) = params
                    .get(argument_index)
                    .and_then(|parameter| pat_binding_key(parameter))
                {
                    self.note_alias(parameter_key, argument_key);
                }
                continue;
            }

            // A spread can contribute any number of arguments. A later fixed
            // argument may therefore feed any parameter from its minimum
            // position onward; keep every such source callable.
            let minimum_parameter_index = argument_index - spreads_seen;
            for parameter in params.iter().skip(minimum_parameter_index) {
                if let Some(parameter_key) = pat_binding_key(parameter) {
                    self.note_alias(parameter_key, argument_key.clone());
                }
            }
        }
    }

    fn record_iife_result_alias(&mut self, target: BindingKey, init: &Expr) {
        let Expr::Call(call) = strip_parens(init) else {
            return;
        };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };

        let mut returns = IifeReturnCollector::default();
        let mut value_returns = ValueReturnCollector::default();
        match strip_parens(callee) {
            Expr::Fn(function) => {
                let Some(body) = &function.function.body else {
                    return;
                };
                body.visit_with(&mut returns);
                body.visit_with(&mut value_returns);
            }
            Expr::Arrow(arrow) => match arrow.body.as_ref() {
                ArrowFunctionBody::FunctionBody(body) => {
                    body.visit_with(&mut returns);
                    body.visit_with(&mut value_returns);
                }
                ArrowFunctionBody::Expr(expr) => {
                    if let Some(source) = expr_binding_key(strip_parens(expr)) {
                        returns.bindings.insert(source);
                    }
                    value_returns.bindings.extend(value_alias_bindings(expr));
                }
            },
            _ => return,
        }

        // Bare `return ident` stays an ordinary alias so an export root can
        // still reach that constructor. Comma / `||` tails are value aliases.
        for source in &returns.bindings {
            self.note_alias(target.clone(), source.clone());
        }
        for source in value_returns.bindings {
            if source == target || returns.bindings.contains(&source) {
                continue;
            }
            self.note_value_alias(target.clone(), source);
        }
    }

    /// `var t = e` / `t = e`, plus comma-tail and `||` values of that initializer.
    /// A bare ident stays an ordinary alias. Value edges do not unwrap assignments:
    /// `Fallback = Ctor` is its own assignment and already records `Fallback → Ctor`.
    fn record_direct_ident_alias(&mut self, target: BindingKey, init: &Expr) {
        let bare = expr_binding_key(strip_parens(init));
        if let Some(source) = &bare {
            if source != &target {
                self.note_alias(target.clone(), source.clone());
            }
        }
        for source in value_alias_bindings(init) {
            if source == target || bare.as_ref() == Some(&source) {
                continue;
            }
            self.note_value_alias(target.clone(), source);
        }
    }

    fn in_constructor(&self) -> bool {
        self.in_constructor.last().copied().unwrap_or(false)
    }

    /// Walk a blankable class IIFE without tagging every nested `.call`.
    ///
    /// Class recovery deletes the wrapper, the parameter alias, and constructor
    /// `super` calls. Calls in methods stay on the class and must still pin.
    fn visit_blankable_init(&mut self, owner: BindingKey, init: &Expr) {
        let Expr::Call(call) = strip_parens(init) else {
            init.visit_with(self);
            return;
        };
        self.guard_stack.push(owner.clone());
        self.record_iife_param_aliases(call);
        self.guard_stack.pop();

        self.iife_scopes.push(IifeConsumeScope {
            owner,
            super_param: single_iife_param(call),
            saw_constructor: false,
            base_depth: self.function_depth,
        });
        for arg in &call.args {
            arg.visit_with(self);
        }
        if let Callee::Expr(callee) = &call.callee {
            callee.visit_with(self);
        }
        self.iife_scopes.pop();
    }
}

impl Visit for CallabilityCollector {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        let name = pat_binding_key(&declarator.name);
        let owner = name
            .clone()
            .filter(|key| self.record_effects && self.blankable.contains(key));
        // The result alias disappears with the wrapper. Nested calls do not.
        if let Some(owner) = &owner {
            self.guard_stack.push(owner.clone());
        }
        if let (Some(target), Some(init)) = (name, declarator.init.as_deref()) {
            self.record_iife_result_alias(target.clone(), init);
            self.record_direct_ident_alias(target, init);
        }
        if owner.is_some() {
            self.guard_stack.pop();
        }
        declarator.name.visit_with(self);
        if let Some(init) = declarator.init.as_deref() {
            if let Some(owner) = owner {
                self.visit_blankable_init(owner, init);
            } else {
                init.visit_with(self);
            }
        }
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        if let Some(scope) = self.iife_scopes.last_mut() {
            if self.function_depth == scope.base_depth + 1 && !scope.saw_constructor {
                scope.saw_constructor = true;
                self.pending_constructor = true;
            }
        }
        decl.visit_children_with(self);
    }

    fn visit_function(&mut self, function: &Function) {
        let constructor = self.pending_constructor;
        self.pending_constructor = false;
        self.in_constructor.push(constructor);
        self.function_depth += 1;
        function.visit_children_with(self);
        self.function_depth -= 1;
        self.in_constructor.pop();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        // Constructor rewriting does not enter arrows.
        self.pending_constructor = false;
        self.in_constructor.push(false);
        self.function_depth += 1;
        arrow.visit_children_with(self);
        self.function_depth -= 1;
        self.in_constructor.pop();
    }

    fn visit_class(&mut self, class: &Class) {
        if let Some(member) = class.super_class.as_deref().and_then(member_key) {
            self.constructed_members.insert(member);
        }
        self.pending_constructor = false;
        self.in_constructor.push(false);
        class.visit_children_with(self);
        self.in_constructor.pop();
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        if assignment.op == AssignOp::Assign {
            if let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assignment.left {
                let key = binding_key(&target.id);
                self.record_iife_result_alias(key.clone(), &assignment.right);
                self.record_direct_ident_alias(key, &assignment.right);
            }
        }
        assignment.visit_children_with(self);
    }

    fn visit_new_expr(&mut self, new: &NewExpr) {
        if let Some(member) = member_key(&new.callee) {
            self.constructed_members.insert(member);
        }
        new.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if member_prop_name(&member.prop, "prototype") {
            if let Some(owner_member) = member_key(&member.obj) {
                self.constructed_members.insert(owner_member);
            }
        }
        member.visit_children_with(self);
    }

    fn visit_bin_expr(&mut self, bin: &BinExpr) {
        if bin.op == BinaryOp::InstanceOf {
            if let Some(member) = member_key(&bin.right) {
                self.constructed_members.insert(member);
            }
        }
        bin.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(binding) = ordinary_call_target(call) {
            let consumed = self.in_constructor()
                && self
                    .iife_scopes
                    .last()
                    .and_then(|scope| scope.super_param.as_ref())
                    == Some(&binding)
                && constructor_super_call_is_rewritten(call);
            if consumed {
                if let Some(owner) = self.iife_scopes.last().map(|scope| scope.owner.clone()) {
                    self.guard_stack.push(owner);
                    self.note_required(binding);
                    self.guard_stack.pop();
                } else {
                    self.note_required(binding);
                }
            } else {
                self.note_required(binding);
            }
        }
        self.record_iife_param_aliases(call);
        call.visit_children_with(self);
    }
}

/// `F.call` / `F.apply`, or the callee of `F.call.apply(G, …)` which is `G`.
pub(crate) fn ordinary_call_target(call: &CallExpr) -> Option<BindingKey> {
    call_apply_argument_binding(call).or_else(|| ident_call_or_apply_binding(call))
}

/// `F.call.apply(G, …)` — `[[Call]]` belongs to `G`, matching argument spread.
fn call_apply_argument_binding(call: &CallExpr) -> Option<BindingKey> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(outer) = strip_parens(callee) else {
        return None;
    };
    if !member_prop_name(&outer.prop, "apply") {
        return None;
    }
    let Expr::Member(inner) = strip_parens(&outer.obj) else {
        return None;
    };
    if !member_prop_name(&inner.prop, "call") {
        return None;
    }
    let argument = call.args.first()?;
    if argument.spread.is_some() {
        return None;
    }
    let Expr::Ident(ident) = strip_parens(&argument.expr) else {
        return None;
    };
    Some(binding_key(ident))
}

fn single_iife_param(call: &CallExpr) -> Option<BindingKey> {
    if call.args.len() != 1 {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    match strip_parens(callee) {
        Expr::Fn(function) => match function.function.params.as_slice() {
            [param] => pat_binding_key(&param.pat),
            _ => None,
        },
        Expr::Arrow(arrow) => match arrow.params.as_slice() {
            [pat] => pat_binding_key(pat),
            _ => None,
        },
        _ => None,
    }
}

/// `SuperCallRewriter` turns these constructor calls into `super(...)`.
fn constructor_super_call_is_rewritten(call: &CallExpr) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return false;
    };
    let Some(first) = call.args.first() else {
        return false;
    };
    if first.spread.is_some() || !matches!(strip_parens(&first.expr), Expr::This(_)) {
        return false;
    }
    if member_prop_name(&member.prop, "call") {
        return true;
    }
    if !member_prop_name(&member.prop, "apply")
        || call.args.len() != 2
        || call.args[1].spread.is_some()
    {
        return false;
    }
    let second = strip_parens(&call.args[1].expr);
    matches!(second, Expr::Ident(id) if id.sym.as_ref() == "arguments")
        || matches!(second, Expr::Array(_))
}

/// Call and alias facts, tagged with the blankable var bindings that contain them.
pub(crate) fn module_guarded_call_effects(
    items: &[ModuleItem],
    blankable: &HashSet<BindingKey>,
) -> Vec<GuardedCallEffect> {
    let mut collector = CallabilityCollector {
        record_effects: true,
        blankable: blankable.clone(),
        ..CallabilityCollector::default()
    };
    items.visit_with(&mut collector);
    collector.effects
}

/// Required bindings after dropping effects guarded by `blanked`.
///
/// `.call` / `.apply` seeds close through ordinary aliases and value aliases.
/// `roots` close through ordinary aliases only, matching [`collect`].
pub(crate) fn required_bindings_from_effects(
    effects: &[GuardedCallEffect],
    blanked: &HashSet<BindingKey>,
    roots: &HashSet<BindingKey>,
) -> HashSet<BindingKey> {
    let mut call_seeds = HashSet::default();
    let mut ordinary: HashMap<BindingKey, Vec<BindingKey>> = HashMap::default();
    let mut value: HashMap<BindingKey, Vec<BindingKey>> = HashMap::default();
    for effect in effects {
        if effect.guards.iter().any(|guard| blanked.contains(guard)) {
            continue;
        }
        match &effect.kind {
            GuardedCallEffectKind::Required(binding) => {
                call_seeds.insert(binding.clone());
            }
            GuardedCallEffectKind::Alias { target, source } => {
                ordinary
                    .entry(target.clone())
                    .or_default()
                    .push(source.clone());
            }
            GuardedCallEffectKind::ValueAlias { target, source } => {
                value
                    .entry(target.clone())
                    .or_default()
                    .push(source.clone());
            }
        }
    }

    close_required(&mut call_seeds, &ordinary, &value, true);
    let mut from_roots = roots.clone();
    close_required(&mut from_roots, &ordinary, &value, false);
    call_seeds.extend(from_roots);
    call_seeds
}

fn alias_map(pairs: &[(BindingKey, BindingKey)]) -> HashMap<BindingKey, Vec<BindingKey>> {
    let mut sources_by_target: HashMap<BindingKey, Vec<BindingKey>> = HashMap::default();
    for (target, source) in pairs {
        sources_by_target
            .entry(target.clone())
            .or_default()
            .push(source.clone());
    }
    sources_by_target
}

/// `walk_value` is set for `.call` / `.apply` seeds. Export roots pass `false`
/// so a decorator `||` cannot pin a constructor that nobody calls.
fn close_required(
    required: &mut HashSet<BindingKey>,
    ordinary: &HashMap<BindingKey, Vec<BindingKey>>,
    value: &HashMap<BindingKey, Vec<BindingKey>>,
    walk_value: bool,
) {
    let mut pending: Vec<BindingKey> = required.iter().cloned().collect();
    while let Some(target) = pending.pop() {
        if let Some(sources) = ordinary.get(&target) {
            for source in sources {
                if required.insert(source.clone()) {
                    pending.push(source.clone());
                }
            }
        }
        if walk_value {
            if let Some(sources) = value.get(&target) {
                for source in sources {
                    if required.insert(source.clone()) {
                        pending.push(source.clone());
                    }
                }
            }
        }
    }
}

/// Idents a value expression may evaluate to, for a live `.call` seed.
///
/// Parens are stripped. A comma sequence contributes only its last item.
/// `||` contributes both sides, and a side that is itself `||` is walked
/// again. A plain `=` assignment contributes its right side, so a CommonJS
/// chain `var Ctor = exports.Ctor = Decorator(…) || Fallback` reaches
/// `Fallback`. Calls, members, conditionals, `&&`, `??`, compound
/// assignments, functions, arrows, and classes contribute nothing.
fn value_alias_bindings(expr: &Expr) -> Vec<BindingKey> {
    fn walk(expr: &Expr, out: &mut Vec<BindingKey>) {
        match strip_parens(expr) {
            Expr::Ident(ident) => out.push(binding_key(ident)),
            Expr::Seq(sequence) => {
                if let Some(last) = sequence.exprs.last() {
                    walk(last, out);
                }
            }
            Expr::Bin(binary) if binary.op == BinaryOp::LogicalOr => {
                walk(&binary.left, out);
                walk(&binary.right, out);
            }
            Expr::Assign(assign) if assign.op == AssignOp::Assign => walk(&assign.right, out),
            _ => {}
        }
    }

    let mut bindings = Vec::new();
    walk(expr, &mut bindings);
    bindings
}

fn pat_binding_key(pat: &Pat) -> Option<BindingKey> {
    let Pat::Ident(BindingIdent { id, .. }) = pat else {
        return None;
    };
    Some(binding_key(id))
}

fn ident_call_or_apply_binding(call: &CallExpr) -> Option<BindingKey> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    if !member_prop_name(&member.prop, "call") && !member_prop_name(&member.prop, "apply") {
        return None;
    }
    let Expr::Ident(ident) = strip_parens(&member.obj) else {
        return None;
    };
    Some(binding_key(ident))
}

#[derive(Default)]
struct IifeReturnCollector {
    bindings: HashSet<BindingKey>,
}

impl Visit for IifeReturnCollector {
    fn visit_return_stmt(&mut self, statement: &ReturnStmt) {
        let Some(argument) = statement.arg.as_deref() else {
            return;
        };
        if let Some(binding) = expr_binding_key(strip_parens(argument)) {
            self.bindings.insert(binding);
        }
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_class(&mut self, _: &Class) {}
}

/// Comma / `||` idents returned from an IIFE, without entering nested functions.
#[derive(Default)]
struct ValueReturnCollector {
    bindings: Vec<BindingKey>,
}

impl Visit for ValueReturnCollector {
    fn visit_return_stmt(&mut self, statement: &ReturnStmt) {
        let Some(argument) = statement.arg.as_deref() else {
            return;
        };
        self.bindings.extend(value_alias_bindings(argument));
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_class(&mut self, _: &Class) {}
}

/// Bindings reachable by IIFE-return aliases from `roots`, including `roots`.
///
/// Natural `.call` sites are not seeds. Nested class recovery uses this so a
/// pinned `var Outer = (function () { return Foo })()` also pins `Foo`.
/// Value aliases are not included: an export root must not cross a decorator
/// `||` or a comma tail.
pub(crate) fn alias_closure_of_roots(
    items: &[ModuleItem],
    roots: &HashSet<BindingKey>,
) -> HashSet<BindingKey> {
    let mut collector = CallabilityCollector::default();
    items.visit_with(&mut collector);
    let mut required = roots.clone();
    let mut sources_by_target: HashMap<BindingKey, Vec<BindingKey>> = HashMap::default();
    for (target, source) in collector.aliases {
        sources_by_target.entry(target).or_default().push(source);
    }
    // `var t = e` sits between an IIFE return and the constructor. Direct ident
    // aliases and return aliases have to close together, or the chain stops.
    for (target, sources) in direct_ident_alias_sources(items) {
        sources_by_target.entry(target).or_default().extend(sources);
    }
    let mut pending: Vec<BindingKey> = required.iter().cloned().collect();
    while let Some(target) = pending.pop() {
        let Some(sources) = sources_by_target.get(&target) else {
            continue;
        };
        for source in sources {
            if required.insert(source.clone()) {
                pending.push(source.clone());
            }
        }
    }
    required
}

/// Binding keys of `exports` on this module's own export declarations.
/// `export { t as Foo }` contributes `t`, not the exported spelling.
/// `export default (function () { return t })()` with no local contributes `t`.
pub(crate) fn pinned_binding_keys(
    items: &[ModuleItem],
    exports: &HashSet<Atom>,
) -> HashSet<BindingKey> {
    let mut keys = export_binding_keys(items, exports);
    expand_direct_ident_aliases(items, &mut keys);
    keys
}

/// [`pinned_binding_keys`] with one shared direct-ident alias map.
pub(crate) fn pinned_binding_keys_with_alias_sources(
    items: &[ModuleItem],
    exports: &HashSet<Atom>,
    alias_sources: &HashMap<BindingKey, Vec<BindingKey>>,
) -> HashSet<BindingKey> {
    let mut keys = export_binding_keys(items, exports);
    expand_alias_sources(alias_sources, &mut keys);
    keys
}

fn object_ident_property_bindings(
    object: &swc_core::ecma::ast::ObjectLit,
    exports: &HashSet<Atom>,
) -> Vec<BindingKey> {
    let mut keys = Vec::new();
    for prop in &object.props {
        let swc_core::ecma::ast::PropOrSpread::Prop(prop) = prop else {
            continue;
        };
        match prop.as_ref() {
            swc_core::ecma::ast::Prop::Shorthand(ident) if exports.contains(&ident.sym) => {
                keys.push(binding_key(ident));
            }
            swc_core::ecma::ast::Prop::KeyValue(pair) => {
                let name = match &pair.key {
                    swc_core::ecma::ast::PropName::Ident(ident) => Some(ident.sym.clone()),
                    swc_core::ecma::ast::PropName::Str(value) => {
                        value.value.as_str().map(Atom::from)
                    }
                    swc_core::ecma::ast::PropName::Num(_)
                    | swc_core::ecma::ast::PropName::BigInt(_)
                    | swc_core::ecma::ast::PropName::Computed(_) => None,
                };
                if let (Some(name), Expr::Ident(ident)) = (name, strip_parens(&pair.value)) {
                    if exports.contains(&name) {
                        keys.push(binding_key(ident));
                    }
                }
            }
            _ => {}
        }
    }
    keys
}

fn export_binding_keys(items: &[ModuleItem], exports: &HashSet<Atom>) -> HashSet<BindingKey> {
    let mut keys = HashSet::default();
    if exports.is_empty() {
        return keys;
    }
    for item in items {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::ExportDecl(export) => match &export.decl {
                Decl::Fn(function) if exports.contains(&function.ident.sym) => {
                    keys.insert(binding_key(&function.ident));
                }
                Decl::Var(var) => {
                    for declarator in &var.decls {
                        let Some(name) = pat_binding_key(&declarator.name) else {
                            continue;
                        };
                        let Pat::Ident(binding) = &declarator.name else {
                            continue;
                        };
                        if exports.contains(&binding.id.sym) {
                            keys.insert(name);
                        }
                    }
                }
                _ => {}
            },
            ModuleDecl::ExportDefaultDecl(export) if exports.contains(&default_atom()) => {
                match &export.decl {
                    DefaultDecl::Fn(function) => {
                        if let Some(ident) = &function.ident {
                            keys.insert(binding_key(ident));
                        }
                    }
                    DefaultDecl::Class(class) => {
                        if let Some(ident) = &class.ident {
                            keys.insert(binding_key(ident));
                        }
                    }
                    DefaultDecl::TsInterfaceDecl(_) => {}
                }
            }
            ModuleDecl::ExportDefaultExpr(export) => {
                // `export default { Foo: local }` — `mod.Foo` is that local.
                if let Expr::Object(object) = strip_parens(&export.expr) {
                    keys.extend(object_ident_property_bindings(object, exports));
                }
                if exports.contains(&default_atom()) {
                    match strip_parens(&export.expr) {
                        Expr::Ident(ident) => {
                            keys.insert(binding_key(ident));
                        }
                        other => keys.extend(iife_returned_bindings(other)),
                    }
                }
            }
            ModuleDecl::ExportNamed(named) if named.src.is_none() => {
                for specifier in &named.specifiers {
                    match specifier {
                        ExportSpecifier::Named(named_spec) => {
                            let exported = match &named_spec.exported {
                                Some(name) => export_atom(name),
                                None => export_atom(&named_spec.orig),
                            };
                            if !exports.contains(&exported) {
                                continue;
                            }
                            if let ModuleExportName::Ident(ident) = &named_spec.orig {
                                keys.insert(binding_key(ident));
                            }
                        }
                        ExportSpecifier::Default(default_spec)
                            if exports.contains(&default_atom()) =>
                        {
                            keys.insert(binding_key(&default_spec.exported));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    keys
}

/// `var Foo = e` / `Foo = e` on a pinned export: the constructor is `e`.
/// This stays local to pin roots. Ordinary `const alias = Imported; alias.call`
/// is still not a cross-file pin.
fn expand_direct_ident_aliases(items: &[ModuleItem], keys: &mut HashSet<BindingKey>) {
    if keys.is_empty() {
        return;
    }
    let sources = direct_ident_alias_sources(items);
    expand_alias_sources(&sources, keys);
}

pub(crate) fn direct_ident_alias_sources(
    items: &[ModuleItem],
) -> HashMap<BindingKey, Vec<BindingKey>> {
    let mut aliases = DirectIdentAliasCollector::default();
    items.visit_with(&mut aliases);
    aliases.sources
}

fn expand_alias_sources(
    sources: &HashMap<BindingKey, Vec<BindingKey>>,
    keys: &mut HashSet<BindingKey>,
) {
    if keys.is_empty() {
        return;
    }
    let mut pending: Vec<BindingKey> = keys.iter().cloned().collect();
    while let Some(target) = pending.pop() {
        let Some(list) = sources.get(&target) else {
            continue;
        };
        for source in list {
            if keys.insert(source.clone()) {
                pending.push(source.clone());
            }
        }
    }
}

#[derive(Default)]
struct DirectIdentAliasCollector {
    sources: HashMap<BindingKey, Vec<BindingKey>>,
}

impl DirectIdentAliasCollector {
    fn record(&mut self, target: BindingKey, init: &Expr) {
        let Some(source) = expr_binding_key(strip_parens(init)) else {
            return;
        };
        self.sources.entry(target).or_default().push(source);
    }
}

impl Visit for DirectIdentAliasCollector {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Some(target), Some(init)) = (
            pat_binding_key(&declarator.name),
            declarator.init.as_deref(),
        ) {
            self.record(target, init);
        }
        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        if assignment.op == AssignOp::Assign {
            if let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assignment.left {
                self.record(binding_key(&target.id), &assignment.right);
            }
        }
        assignment.visit_children_with(self);
    }
}

fn default_atom() -> Atom {
    Atom::from("default")
}

fn export_atom(name: &ModuleExportName) -> Atom {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.clone(),
        ModuleExportName::Str(value) => Atom::from(value.value.as_str().unwrap_or("")),
    }
}

fn iife_returned_bindings(expr: &Expr) -> HashSet<BindingKey> {
    let Expr::Call(call) = strip_parens(expr) else {
        return HashSet::default();
    };
    let Callee::Expr(callee) = &call.callee else {
        return HashSet::default();
    };
    let mut returns = IifeReturnCollector::default();
    match strip_parens(callee) {
        Expr::Fn(function) => {
            if let Some(body) = &function.function.body {
                body.visit_with(&mut returns);
            }
        }
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            ArrowFunctionBody::FunctionBody(body) => body.visit_with(&mut returns),
            ArrowFunctionBody::Expr(expr) => {
                if let Some(source) = expr_binding_key(strip_parens(expr)) {
                    returns.bindings.insert(source);
                }
            }
        },
        _ => {}
    }
    returns.bindings
}
