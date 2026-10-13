//! Moves the requires a top-level statement runs before any other effect
//! into `const` bindings placed right before the statement, so `UnEsm` can
//! turn them into imports in a module that is not webpack harmony output.
//!
//! Source code and minifiers both leave requires inside an expression:
//! `module.exports = { a: require("./a"), b: require("./b") }` as written,
//! or a bundled interop helper's call that Terser inlined into its single
//! use, `(0, h(require("./x")).default)(Error)`. When everything the
//! statement evaluates before such a require has no effect (a literal, a
//! name, a member read, a function expression), the require already runs
//! first, so a binding placed before the statement runs it at the same
//! point. The binding then becomes an import under
//! `import_hoisting_eagerness`, as a top-level `var x = require(p)` does.
//! The hoisting barrier has already hidden every require it keeps in place,
//! so none of those moves.
//!
//! Two shapes move:
//!
//! - `require("./p")` in a value position: a property value, an array
//!   element, or a call argument. One that is called, has a member called,
//!   or is assigned keeps its own handling.
//! - a call of a name or of a member of a name around one,
//!   `h(require("./p"))`, in any position. It moves as a unit, the top-level
//!   binding of a bundled interop helper that a later unpack stage converts
//!   (`cross_module_interop_import`).

use crate::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinaryOp, CallExpr, Callee, Decl, Expr, Ident, Lit, MemberProp, Module,
    ModuleItem, Pat, Prop, PropName, PropOrSpread, SimpleAssignTarget, Stmt, UnaryOp,
};

use crate::kept_require::is_kept_require_call;
use crate::rules::decl_utils::{fresh_binding_ident, fresh_name_from, fresh_specifier_name};
use crate::utils::paren::strip_parens;

use super::{collect_all_declared_names, collect_unresolved_reference_names, make_var_item};

/// Gives each require that a top-level statement runs before any other
/// effect a `const` binding right before the statement.
pub(super) fn hoist_leading_requires(module: &mut Module, unresolved_mark: Mark) {
    let mut used_names = collect_all_declared_names(module);
    used_names.extend(collect_unresolved_reference_names(module, unresolved_mark));
    let mut body = Vec::with_capacity(module.body.len());
    for mut item in std::mem::take(&mut module.body) {
        if let ModuleItem::Stmt(stmt) = &mut item {
            let mut hoister = LeadingRequires {
                unresolved_mark,
                used_names: &mut used_names,
                inserted: Vec::new(),
                stopped: false,
            };
            hoister.stmt(stmt);
            body.extend(
                hoister
                    .inserted
                    .into_iter()
                    .map(|(local, init)| make_var_item(local, init)),
            );
        }
        body.push(item);
    }
    module.body = body;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    /// A property value, an array element, or a call argument: the value
    /// is only passed on.
    Value,
    Other,
}

struct LeadingRequires<'a> {
    unresolved_mark: Mark,
    used_names: &'a mut HashSet<Atom>,
    inserted: Vec<(Ident, Box<Expr>)>,
    /// The walk met an effect that is not a moved require: nothing after it
    /// may move.
    stopped: bool,
}

impl LeadingRequires<'_> {
    fn stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::Expr(expr) => self.expr(&mut expr.expr, Position::Other, None),
            Stmt::Decl(Decl::Var(var)) => {
                for declarator in &mut var.decls {
                    if self.stopped {
                        return;
                    }
                    let Some(init) = declarator.init.as_deref_mut() else {
                        continue;
                    };
                    // `var x = require(p)` and its member and helper forms
                    // convert as they are; the require is this declarator's
                    // effect.
                    if self.is_bound_require(init) {
                        continue;
                    }
                    self.expr(init, Position::Other, None);
                    // Destructuring reads properties of the value.
                    if !matches!(declarator.name, Pat::Ident(_)) {
                        self.stopped = true;
                    }
                }
            }
            // Function and class declarations run nothing; a require in any
            // other statement runs conditionally or repeatedly.
            _ => {}
        }
    }

    /// `require(p)`, `require(p).a.b`, or `h(require(p))`.
    fn is_bound_require(&self, init: &Expr) -> bool {
        let mut current = strip_parens(init);
        while let Expr::Member(member) = current {
            current = strip_parens(&member.obj);
        }
        match current {
            Expr::Call(call) => {
                self.string_require(call).is_some() || self.helper_unit(call).is_some()
            }
            _ => false,
        }
    }

    fn string_require(&self, call: &CallExpr) -> Option<Atom> {
        if !is_kept_require_call(call, self.unresolved_mark) {
            return None;
        }
        match call.args.as_slice() {
            [arg] if arg.spread.is_none() => match strip_parens(&arg.expr) {
                Expr::Lit(Lit::Str(source)) => source.value.as_str().map(Atom::from),
                _ => None,
            },
            _ => None,
        }
    }

    /// The source of `h(require(p))`, `ns.h(require(p))`, or
    /// `(0, ns.h)(require(p))`: a call of a name or a member of a name whose
    /// only argument is a string require.
    fn helper_unit(&self, call: &CallExpr) -> Option<Atom> {
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let is_named = |expr: &Expr| match strip_parens(expr) {
            Expr::Ident(id) => id.sym != "require" || id.ctxt.outer() != self.unresolved_mark,
            Expr::Member(member) => {
                matches!(strip_parens(&member.obj), Expr::Ident(_))
                    && matches!(member.prop, MemberProp::Ident(_))
            }
            _ => false,
        };
        let named = match strip_parens(callee) {
            Expr::Seq(seq) => {
                seq.exprs.len() == 2
                    && matches!(&*seq.exprs[0], Expr::Lit(Lit::Num(_)))
                    && is_named(&seq.exprs[1])
            }
            callee => is_named(callee),
        };
        if !named {
            return None;
        }
        match call.args.as_slice() {
            [arg] if arg.spread.is_none() => match strip_parens(&arg.expr) {
                Expr::Call(inner) => self.string_require(inner),
                _ => None,
            },
            _ => None,
        }
    }

    /// The source when `expr` moves as a whole.
    fn movable(&self, expr: &Expr, position: Position) -> Option<Atom> {
        let Expr::Call(call) = strip_parens(expr) else {
            return None;
        };
        if position == Position::Value {
            if let Some(source) = self.string_require(call) {
                return Some(source);
            }
        }
        self.helper_unit(call)
    }

    fn hoist(&mut self, expr: &mut Expr, source: &str, name_hint: Option<&str>) {
        // The property key names the value; `fresh_name_from` makes a key
        // like `"beta-gamma"` a binding name.
        let name = match name_hint {
            Some(hint) if hint.chars().any(|c| c.is_ascii_alphabetic()) => {
                fresh_name_from(hint, self.used_names)
            }
            _ => fresh_specifier_name(source, self.used_names),
        };
        let local = fresh_binding_ident(name, DUMMY_SP);
        let init = std::mem::replace(expr, Expr::Ident(local.clone()));
        self.inserted.push((local, Box::new(init)));
    }

    fn stop(&mut self) {
        self.stopped = true;
    }

    /// Walks `expr` in evaluation order, moving each movable require it
    /// meets before the first other effect.
    fn expr(&mut self, expr: &mut Expr, position: Position, name_hint: Option<&str>) {
        if self.stopped {
            return;
        }
        if let Some(source) = self.movable(expr, position) {
            self.hoist(expr, &source, name_hint);
            return;
        }
        match expr {
            Expr::Lit(_) | Expr::Ident(_) | Expr::This(_) | Expr::Fn(_) | Expr::Arrow(_) => {}
            Expr::Paren(paren) => self.expr(&mut paren.expr, position, name_hint),
            Expr::Seq(seq) => {
                for element in &mut seq.exprs {
                    self.expr(element, Position::Other, None);
                }
            }
            Expr::Array(array) => {
                for element in array.elems.iter_mut().flatten() {
                    if element.spread.is_some() {
                        // Spreading runs the iterator.
                        self.expr(&mut element.expr, Position::Other, None);
                        self.stop();
                    } else {
                        self.expr(&mut element.expr, Position::Value, None);
                    }
                }
            }
            Expr::Object(object) => {
                for prop in &mut object.props {
                    match prop {
                        PropOrSpread::Spread(spread) => {
                            // Spreading reads every property.
                            self.expr(&mut spread.expr, Position::Other, None);
                            self.stop();
                        }
                        PropOrSpread::Prop(prop) => match &mut **prop {
                            Prop::Shorthand(_) => {}
                            Prop::KeyValue(key_value) => {
                                self.key(&mut key_value.key);
                                let hint = match &key_value.key {
                                    PropName::Ident(name) => Some(name.sym.to_string()),
                                    PropName::Str(name) => name.value.as_str().map(str::to_owned),
                                    _ => None,
                                };
                                self.expr(&mut key_value.value, Position::Value, hint.as_deref());
                            }
                            Prop::Method(method) => self.key(&mut method.key),
                            Prop::Getter(getter) => self.key(&mut getter.key),
                            Prop::Setter(setter) => self.key(&mut setter.key),
                            Prop::Assign(_) => self.stop(),
                        },
                    }
                }
            }
            // A member read is no effect here, as for the hoisting barrier.
            Expr::Member(member) => {
                self.expr(&mut member.obj, Position::Other, None);
                if let MemberProp::Computed(computed) = &mut member.prop {
                    self.expr(&mut computed.expr, Position::Other, None);
                }
            }
            Expr::Call(call) => {
                if let Callee::Expr(callee) = &mut call.callee {
                    self.expr(callee, Position::Other, None);
                } else {
                    self.stop();
                }
                for arg in &mut call.args {
                    if arg.spread.is_some() {
                        self.expr(&mut arg.expr, Position::Other, None);
                        self.stop();
                    } else {
                        self.expr(&mut arg.expr, Position::Value, None);
                    }
                }
                self.stop();
            }
            Expr::New(new) => {
                self.expr(&mut new.callee, Position::Other, None);
                for arg in new.args.iter_mut().flatten() {
                    if arg.spread.is_some() {
                        self.expr(&mut arg.expr, Position::Other, None);
                        self.stop();
                    } else {
                        self.expr(&mut arg.expr, Position::Value, None);
                    }
                }
                self.stop();
            }
            Expr::Assign(assign) if assign.op == AssignOp::Assign => {
                match &mut assign.left {
                    AssignTarget::Simple(SimpleAssignTarget::Ident(_)) => {}
                    AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                        self.expr(&mut member.obj, Position::Other, None);
                        if let MemberProp::Computed(computed) = &mut member.prop {
                            self.expr(&mut computed.expr, Position::Other, None);
                        }
                    }
                    _ => self.stop(),
                }
                // No name from the target: `const y = …; exports.y = y`
                // would become `export const y = h(require(p))`, a form no
                // later unpack stage reads.
                self.expr(&mut assign.right, Position::Other, None);
                self.stop();
            }
            // `typeof`, `void`, and `!` convert nothing; `+x` and the like
            // may call `valueOf`.
            Expr::Unary(unary) if unary.op != UnaryOp::Delete => {
                self.expr(&mut unary.arg, Position::Other, None);
                if !matches!(unary.op, UnaryOp::TypeOf | UnaryOp::Void | UnaryOp::Bang) {
                    self.stop();
                }
            }
            // The right side of a logical operator runs conditionally, and
            // other operators may convert their operands.
            Expr::Bin(binary) => {
                self.expr(&mut binary.left, Position::Other, None);
                if !matches!(
                    binary.op,
                    BinaryOp::LogicalAnd | BinaryOp::LogicalOr | BinaryOp::NullishCoalescing
                ) {
                    self.expr(&mut binary.right, Position::Other, None);
                }
                self.stop();
            }
            Expr::Cond(cond) => {
                self.expr(&mut cond.test, Position::Other, None);
                self.stop();
            }
            _ => self.stop(),
        }
    }

    fn key(&mut self, key: &mut PropName) {
        if let PropName::Computed(computed) = key {
            self.expr(&mut computed.expr, Position::Other, None);
            // A computed key converts its value to a property key.
            self.stop();
        }
    }
}
