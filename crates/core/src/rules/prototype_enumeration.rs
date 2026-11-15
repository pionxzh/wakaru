//! Constructors whose methods a scope can enumerate.
//!
//! `UnPrototypeClass` turns `F.prototype.m = function () {}` into a class
//! method, which is not enumerable, and `F.m = function () {}` into a static
//! method, which is not enumerable either. A scope that enumerates the
//! prototype, an instance, or the constructor's own properties would lose
//! those members (docs/rewrite-assumptions.md,
//! `prototype_methods_not_enumerated`). This walk finds the constructors such
//! a scope reaches:
//!
//! - `for (k in F.prototype)`, `for (k in new F())`, `for (k in F)`, and
//!   `for (k in x)` where `x` is assigned `new F(…)`;
//! - `for (k in this)` in the constructor or a method assigned to
//!   `F.prototype.m` (an arrow keeps the enclosing `this`);
//! - `F.prototype` used as a value: passed to a call (a mixin,
//!   `Object.assign`, `Object.keys`), stored, or returned. A variable that
//!   holds it (`var proto = F.prototype`) stands for it: `proto.m = …` is a
//!   member write, `for (k in proto)` or `mixin(proto)` a use. `Object.create`,
//!   `Object.defineProperty`, `Object.defineProperties`, and
//!   `Object.setPrototypeOf` take it to build or extend a prototype, and
//!   `Object.assign(F.prototype, …)` adds to it; these do
//!   not count, nor does a member access `F.prototype.k` or an identity
//!   comparison `x === F.prototype`;
//! - `Object.keys`, `values`, or `entries` given `F` itself, or
//!   `Object.assign` copying from it.
//!
//! Enumeration the scope cannot see (another module iterating an instance)
//! is what the assumption accepts.

use crate::collections::{HashMap, HashSet};

use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, CallExpr, Callee, Expr, FnDecl,
    ForInStmt, Function, MemberExpr, MemberProp, ModuleItem, Pat, SimpleAssignTarget, Stmt,
    VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::utils::paren::strip_parens;

use super::helper_matcher::{binding_key, BindingKey};

/// Constructor bindings (and other bindings named in an enumeration, which
/// are harmless extras) whose members the scope may enumerate.
pub(crate) struct EnumeratedConstructors {
    bindings: HashSet<BindingKey>,
}

impl EnumeratedConstructors {
    pub(crate) fn collect_module_items(items: &[ModuleItem]) -> Self {
        Self::collect(items)
    }

    pub(crate) fn collect_stmts(stmts: &[Stmt]) -> Self {
        Self::collect(stmts)
    }

    fn collect<N: VisitWith<PrototypeAliases> + VisitWith<Collector>>(nodes: &[N]) -> Self {
        let mut aliases = PrototypeAliases::default();
        for node in nodes {
            node.visit_with(&mut aliases);
        }
        let mut collector = Collector {
            prototype_aliases: aliases.0,
            ..Collector::default()
        };
        for node in nodes {
            node.visit_with(&mut collector);
        }
        collector.finish()
    }

    pub(crate) fn contains(&self, binding: &BindingKey) -> bool {
        self.bindings.contains(binding)
    }
}

/// `proto = F.prototype` (declared or assigned): proto → F.
#[derive(Default)]
struct PrototypeAliases(HashMap<BindingKey, BindingKey>);

impl Visit for PrototypeAliases {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(name), Some(init)) = (&declarator.name, &declarator.init) {
            if let Some(owner) = prototype_owner(init) {
                self.0.insert(binding_key(&name.id), owner);
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let (AssignOp::Assign, AssignTarget::Simple(SimpleAssignTarget::Ident(target))) =
            (assign.op, &assign.left)
        {
            if let Some(owner) = prototype_owner(&assign.right) {
                self.0.insert(binding_key(&target.id), owner);
            }
        }
        assign.visit_children_with(self);
    }
}

#[derive(Default)]
struct Collector {
    prototype_aliases: HashMap<BindingKey, BindingKey>,
    enumerated: HashSet<BindingKey>,
    /// Bindings enumerated with `for…in`; an instance variable among them
    /// marks the constructors it was assigned from.
    for_in_bindings: Vec<BindingKey>,
    /// `x = new F(…)`: x → every F.
    instances: HashMap<BindingKey, HashSet<BindingKey>>,
    /// The constructor whose instance `this` is in each enclosing function.
    this_owner: Vec<Option<BindingKey>>,
    /// Owner for the next function visited.
    pending_owner: Option<BindingKey>,
}

impl Collector {
    /// `F.prototype`, or a variable holding it → F.
    fn prototype_of(&self, expr: &Expr) -> Option<BindingKey> {
        match strip_parens(expr) {
            Expr::Ident(ident) => self.prototype_aliases.get(&binding_key(ident)).cloned(),
            expr => prototype_owner(expr),
        }
    }

    fn finish(mut self) -> EnumeratedConstructors {
        for binding in &self.for_in_bindings {
            if let Some(constructors) = self.instances.get(binding) {
                self.enumerated.extend(constructors.iter().cloned());
            }
        }
        self.enumerated.extend(self.for_in_bindings);
        EnumeratedConstructors {
            bindings: self.enumerated,
        }
    }

    fn note_instance(&mut self, target: BindingKey, value: &Expr) {
        if let Expr::New(new) = strip_parens(value) {
            if let Expr::Ident(constructor) = strip_parens(&new.callee) {
                self.instances
                    .entry(target)
                    .or_default()
                    .insert(binding_key(constructor));
            }
        }
    }
}

/// `F.prototype` → F.
fn prototype_owner(expr: &Expr) -> Option<BindingKey> {
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    member_prototype_owner(member)
}

fn member_prototype_owner(member: &MemberExpr) -> Option<BindingKey> {
    let Expr::Ident(owner) = strip_parens(&member.obj) else {
        return None;
    };
    let is_prototype = match &member.prop {
        MemberProp::Ident(name) => name.sym == "prototype",
        MemberProp::Computed(computed) => matches!(
            strip_parens(&computed.expr),
            Expr::Lit(swc_core::ecma::ast::Lit::Str(s)) if s.value == "prototype"
        ),
        MemberProp::PrivateName(_) => false,
    };
    is_prototype.then(|| binding_key(owner))
}

/// `Object.<name>` callee name.
fn object_method(call: &CallExpr) -> Option<&str> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    let Expr::Ident(object) = strip_parens(&member.obj) else {
        return None;
    };
    if object.sym != "Object" {
        return None;
    }
    match &member.prop {
        MemberProp::Ident(name) => Some(name.sym.as_ref()),
        _ => None,
    }
}

impl Visit for Collector {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        declarator.name.visit_with(self);
        let Some(init) = &declarator.init else {
            return;
        };
        if let Pat::Ident(name) = &declarator.name {
            let target = binding_key(&name.id);
            if self.prototype_aliases.contains_key(&target) && prototype_owner(init).is_some() {
                return;
            }
            self.note_instance(target.clone(), init);
            if matches!(strip_parens(init), Expr::Fn(_)) {
                self.pending_owner = Some(target);
            }
        }
        init.visit_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(target)) => {
                if assign.op == AssignOp::Assign {
                    let target = binding_key(&target.id);
                    if self.prototype_aliases.contains_key(&target)
                        && prototype_owner(&assign.right).is_some()
                    {
                        return;
                    }
                    self.note_instance(target, &assign.right);
                }
            }
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                // `F.prototype = …` replaces the prototype; the rule's own
                // replacement guard handles it. Only the value is a use.
                if member_prototype_owner(member).is_some() {
                    assign.right.visit_with(self);
                    return;
                }
                // `F.prototype.m = function () {…}`: `this` is an F.
                if let Some(owner) = self.prototype_of(&member.obj) {
                    {
                        if matches!(strip_parens(&assign.right), Expr::Fn(_)) {
                            member.prop.visit_with(self);
                            self.pending_owner = Some(owner);
                            assign.right.visit_with(self);
                            return;
                        }
                    }
                }
            }
            _ => {}
        }
        assign.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.pending_owner = Some(binding_key(&decl.ident));
        decl.function.visit_with(self);
    }

    fn visit_function(&mut self, function: &Function) {
        let owner = self.pending_owner.take();
        self.this_owner.push(owner);
        function.visit_children_with(self);
        self.this_owner.pop();
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        self.pending_owner = None;
        arrow.visit_children_with(self);
    }

    fn visit_class(&mut self, class: &swc_core::ecma::ast::Class) {
        self.pending_owner = None;
        class.visit_children_with(self);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        match strip_parens(&stmt.right) {
            Expr::New(new) => {
                if let Expr::Ident(constructor) = strip_parens(&new.callee) {
                    self.enumerated.insert(binding_key(constructor));
                }
            }
            Expr::Ident(ident) => self.for_in_bindings.push(binding_key(ident)),
            Expr::This(_) => {
                if let Some(Some(owner)) = self.this_owner.last() {
                    self.enumerated.insert(owner.clone());
                }
            }
            _ => {}
        }
        // `F.prototype` on the right is a value use, marked below.
        stmt.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        match object_method(call) {
            Some("create" | "defineProperty" | "defineProperties" | "setPrototypeOf") => {
                // The prototype argument builds or extends a prototype.
                call.callee.visit_with(self);
                for arg in &call.args {
                    if arg.spread.is_none() && self.prototype_of(&arg.expr).is_some() {
                        continue;
                    }
                    arg.visit_with(self);
                }
                return;
            }
            Some(name @ ("keys" | "values" | "entries" | "assign")) => {
                // `Object.assign`'s first argument is written, not read.
                let skip = usize::from(name == "assign");
                for arg in call.args.iter().skip(skip) {
                    if let Expr::Ident(ident) = strip_parens(&arg.expr) {
                        self.enumerated.insert(binding_key(ident));
                    }
                }
                // `Object.assign(F.prototype, …)` adds to the prototype.
                if let Some(target) = call.args.first().filter(|arg| {
                    name == "assign"
                        && arg.spread.is_none()
                        && self.prototype_of(&arg.expr).is_some()
                }) {
                    call.callee.visit_with(self);
                    for arg in &call.args {
                        if !std::ptr::eq(arg, target) {
                            arg.visit_with(self);
                        }
                    }
                    return;
                }
            }
            _ => {}
        }
        call.visit_children_with(self);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        // A variable holding `F.prototype` used as a value.
        if let Expr::Ident(ident) = expr {
            if let Some(owner) = self.prototype_aliases.get(&binding_key(ident)) {
                self.enumerated.insert(owner.clone());
            }
            return;
        }
        expr.visit_children_with(self);
    }

    fn visit_bin_expr(&mut self, bin: &BinExpr) {
        // `x === F.prototype` compares identity; it enumerates nothing.
        if matches!(
            bin.op,
            BinaryOp::EqEq | BinaryOp::NotEq | BinaryOp::EqEqEq | BinaryOp::NotEqEq
        ) {
            for side in [&bin.left, &bin.right] {
                if self.prototype_of(side).is_none() {
                    side.visit_with(self);
                }
            }
            return;
        }
        bin.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        // `F.prototype.k` (or `proto.k`): a member of the prototype, not
        // the prototype.
        if self.prototype_of(&member.obj).is_some() {
            member.prop.visit_with(self);
            return;
        }
        if let Some(owner) = member_prototype_owner(member) {
            self.enumerated.insert(owner);
            return;
        }
        member.visit_children_with(self);
    }
}
