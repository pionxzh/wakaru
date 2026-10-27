//! Webpack 4's `buildin/module.js` and `buildin/harmony-module.js`.
//!
//! When a webpack 4 module reads `module` as a value, webpack wraps the module
//! body as `(function (module) { … }).call(this, require(<id>)(module))`,
//! where `<id>` is one of two generated decorator modules. The CommonJS
//! decorator adds Node-like `deprecate`, `paths`, `children`, `loaded`, and
//! `id` members to the module object and returns it; the harmony decorator
//! returns an `Object.create(module)` view with the same members and an
//! undefined `exports`. Both stand in for the host's own `module`, so they
//! are the webpack 4 counterpart of webpack 5's `nmd` / `hmd` decorators,
//! which the webpack 5 normalizer reduces to a plain `module` read.
//!
//! Recovered output runs on a real `module`. Calling the CommonJS decorator
//! there replaces the host's `loaded` with a getter-only property, so the
//! host's own `module.loaded = true` throws once the module finishes. The
//! decorator call is therefore reduced to `module`, the same reduction
//! webpack 5 gets.
//!
//! Production ids are numeric, so a decorator module is recognized by its
//! factory body, never by its name: the body must be exactly
//! `<module>.exports = function (m) { … }` whose every effect is one of the
//! decorator's own member definitions (in the unminified `if` form or the
//! minified `||` / comma form) and whose result is the decorator's object.

use crate::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinaryOp, CallExpr, Callee, Decl, Expr, ExprStmt, Function,
    Ident, Lit, MemberExpr, MemberProp, Module, ObjectLit, Pat, Prop, PropName, PropOrSpread,
    SimpleAssignTarget, Stmt, UnaryOp,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::webpack_common::numeric_id_from_expr;
use crate::utils::paren::strip_parens;

/// Whether a webpack 4 factory with first parameter `module_param` and body
/// `body` is the generated `buildin/module.js` or `buildin/harmony-module.js`
/// decorator.
pub(super) fn is_module_decorator_factory(module_param: Option<&Atom>, body: &[Stmt]) -> bool {
    let (Some(module_param), [Stmt::Expr(ExprStmt { expr, .. })]) = (module_param, body) else {
        return false;
    };
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left: AssignTarget::Simple(SimpleAssignTarget::Member(target)),
        right,
        ..
    }) = strip_parens(expr)
    else {
        return false;
    };
    if !matches!(strip_parens(&target.obj), Expr::Ident(object) if object.sym == *module_param)
        || !member_name_is(&target.prop, "exports")
    {
        return false;
    }
    let Expr::Fn(decorator) = strip_parens(right) else {
        return false;
    };
    decorator.ident.is_none() && is_decorator_function(&decorator.function)
}

/// Reduce every `require(<decorator id>)(module)` to `module`, and report
/// whether any call was reduced. `require` and `module` must be the factory's
/// own (unresolved) runtime bindings, and the id a numeric or string literal
/// naming a recognized decorator module.
pub(super) fn reduce_module_decorator_calls(
    module: &mut Module,
    decorator_ids: &HashSet<String>,
    unresolved_mark: Mark,
) -> bool {
    if decorator_ids.is_empty() {
        return false;
    }
    let mut reducer = DecoratorCallReducer {
        decorator_ids,
        unresolved_mark,
        reduced: false,
    };
    module.visit_mut_with(&mut reducer);
    reducer.reduced
}

struct DecoratorCallReducer<'a> {
    decorator_ids: &'a HashSet<String>,
    unresolved_mark: Mark,
    reduced: bool,
}

impl DecoratorCallReducer<'_> {
    fn decorated_module(&self, call: &CallExpr) -> Option<Ident> {
        let [argument] = call.args.as_slice() else {
            return None;
        };
        if argument.spread.is_some() {
            return None;
        }
        let Expr::Ident(module) = strip_parens(&argument.expr) else {
            return None;
        };
        if !self.is_runtime_ident(module, "module") {
            return None;
        }
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Call(load) = strip_parens(callee) else {
            return None;
        };
        let Callee::Expr(loader) = &load.callee else {
            return None;
        };
        if !matches!(strip_parens(loader), Expr::Ident(loader) if self.is_runtime_ident(loader, "require"))
        {
            return None;
        }
        let [id] = load.args.as_slice() else {
            return None;
        };
        if id.spread.is_some() {
            return None;
        }
        let id = match strip_parens(&id.expr) {
            Expr::Lit(Lit::Str(id)) => id.value.as_str()?.to_string(),
            expr => numeric_id_from_expr(expr)?.to_string(),
        };
        self.decorator_ids.contains(&id).then(|| module.clone())
    }

    fn is_runtime_ident(&self, ident: &Ident, name: &str) -> bool {
        ident.sym == name && ident.ctxt.outer() == self.unresolved_mark
    }
}

impl VisitMut for DecoratorCallReducer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        if let Expr::Call(call) = expr {
            if let Some(module) = self.decorated_module(call) {
                *expr = Expr::Ident(module);
                self.reduced = true;
            }
        }
    }
}

/// The decorator's effects, recorded by kind.
#[derive(Default)]
struct DecoratorEffects {
    /// The object built by `var view = Object.create(input)` (harmony only).
    view: Option<Atom>,
    /// `(object, member)` for each recognized member definition.
    members: Vec<(Atom, &'static str)>,
    /// The value the decorator returns.
    result: Option<Atom>,
}

fn is_decorator_function(function: &Function) -> bool {
    if function.is_async || function.is_generator {
        return false;
    }
    let [param] = function.params.as_slice() else {
        return false;
    };
    let Pat::Ident(input) = &param.pat else {
        return false;
    };
    let Some(body) = &function.body else {
        return false;
    };
    let input = input.id.sym.clone();
    let mut effects = DecoratorEffects::default();
    if !body
        .stmts
        .iter()
        .all(|stmt| collect_stmt(stmt, &input, &mut effects))
    {
        return false;
    }
    let mut members = effects.members;
    members.sort();
    match effects.view {
        // `buildin/module.js`: members on the input, which it returns.
        None => {
            let mut expected = [
                "children",
                "deprecate",
                "id",
                "loaded",
                "paths",
                "webpackPolyfill",
            ]
            .map(|member| (input.clone(), member))
            .to_vec();
            expected.sort();
            effects.result.as_ref() == Some(&input) && members == expected
        }
        // `buildin/harmony-module.js`: members on a view of the input, which
        // it returns.
        Some(view) => {
            let mut expected = ["children", "exports", "id", "loaded", "webpackPolyfill"]
                .map(|member| (view.clone(), member))
                .to_vec();
            expected.sort();
            view != input && effects.result.as_ref() == Some(&view) && members == expected
        }
    }
}

fn collect_stmt(stmt: &Stmt, input: &Atom, effects: &mut DecoratorEffects) -> bool {
    match stmt {
        Stmt::Block(block) => block
            .stmts
            .iter()
            .all(|stmt| collect_stmt(stmt, input, effects)),
        Stmt::Expr(ExprStmt { expr, .. }) => collect_effect(expr, input, effects),
        Stmt::If(if_stmt) => {
            if_stmt.alt.is_none()
                && is_negated_guard(&if_stmt.test, input, effects)
                && collect_stmt(&if_stmt.cons, input, effects)
        }
        Stmt::Decl(Decl::Var(var)) => {
            let [declarator] = var.decls.as_slice() else {
                return false;
            };
            let (Pat::Ident(view), Some(init)) = (&declarator.name, &declarator.init) else {
                return false;
            };
            if effects.view.is_some() || !is_object_create_of(init, input) {
                return false;
            }
            effects.view = Some(view.id.sym.clone());
            true
        }
        Stmt::Return(ret) => {
            let Some(argument) = &ret.arg else {
                return false;
            };
            effects.result.is_none() && collect_result(argument, input, effects)
        }
        _ => false,
    }
}

/// The returned expression: the result, optionally preceded by effects in a
/// comma sequence (`return m.webpackPolyfill || (…), m`).
fn collect_result(expr: &Expr, input: &Atom, effects: &mut DecoratorEffects) -> bool {
    match strip_parens(expr) {
        Expr::Seq(sequence) => {
            let Some((last, rest)) = sequence.exprs.split_last() else {
                return false;
            };
            rest.iter().all(|expr| collect_effect(expr, input, effects))
                && collect_result(last, input, effects)
        }
        Expr::Ident(result) => {
            effects.result = Some(result.sym.clone());
            true
        }
        _ => false,
    }
}

fn collect_effect(expr: &Expr, input: &Atom, effects: &mut DecoratorEffects) -> bool {
    match strip_parens(expr) {
        Expr::Seq(sequence) => sequence
            .exprs
            .iter()
            .all(|expr| collect_effect(expr, input, effects)),
        // `guard || (effects)` is the minified `if (!guard) { effects }`.
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalOr => {
            is_guard(&bin.left, input, effects) && collect_effect(&bin.right, input, effects)
        }
        Expr::Assign(assign) if assign.op == AssignOp::Assign => {
            let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assign.left else {
                return false;
            };
            let Some((object, member)) = object_member(target) else {
                return false;
            };
            let value = strip_parens(&assign.right);
            let recognized = match member {
                "deprecate" => matches!(value, Expr::Fn(f)
                    if f.function.params.is_empty()
                        && f.function.body.as_ref().is_some_and(|body| body.stmts.is_empty())),
                "paths" | "children" => {
                    matches!(value, Expr::Array(array) if array.elems.is_empty())
                }
                "webpackPolyfill" => {
                    matches!(value, Expr::Lit(Lit::Num(number)) if number.value == 1.0)
                }
                _ => false,
            };
            recognized && record_member(object, member, input, effects)
        }
        Expr::Call(call) => {
            let Some((object, member)) = define_property(call) else {
                return false;
            };
            record_member(object, member, input, effects)
        }
        _ => false,
    }
}

/// A member definition lands on the input (`buildin/module.js`) or on the
/// harmony view; record it once.
fn record_member(
    object: Atom,
    member: &'static str,
    input: &Atom,
    effects: &mut DecoratorEffects,
) -> bool {
    let target_is_known = object == *input || effects.view.as_ref() == Some(&object);
    let entry = (object, member);
    if !target_is_known || effects.members.contains(&entry) {
        return false;
    }
    effects.members.push(entry);
    true
}

/// `Object.defineProperty(object, "loaded" | "id", { enumerable: true, get:
/// function () { return object.l | object.i; } })`, or the harmony view's
/// `Object.defineProperty(view, "exports", { enumerable: true })`.
fn define_property(call: &CallExpr) -> Option<(Atom, &'static str)> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(callee) = strip_parens(callee) else {
        return None;
    };
    if !matches!(strip_parens(&callee.obj), Expr::Ident(object) if object.sym == "Object")
        || !member_name_is(&callee.prop, "defineProperty")
    {
        return None;
    }
    let [object, key, descriptor] = call.args.as_slice() else {
        return None;
    };
    if object.spread.is_some() || key.spread.is_some() || descriptor.spread.is_some() {
        return None;
    }
    let Expr::Ident(object) = strip_parens(&object.expr) else {
        return None;
    };
    let Expr::Lit(Lit::Str(key)) = strip_parens(&key.expr) else {
        return None;
    };
    let Expr::Object(descriptor) = strip_parens(&descriptor.expr) else {
        return None;
    };
    let (member, getter_member) = match key.value.as_str()? {
        "loaded" => ("loaded", Some("l")),
        "id" => ("id", Some("i")),
        "exports" => ("exports", None),
        _ => return None,
    };
    is_member_descriptor(descriptor, &object.sym, getter_member)
        .then(|| (object.sym.clone(), member))
}

fn is_member_descriptor(descriptor: &ObjectLit, object: &Atom, getter: Option<&str>) -> bool {
    let mut enumerable = false;
    let mut get = false;
    for prop in &descriptor.props {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        match prop.as_ref() {
            Prop::KeyValue(prop) if prop_name_is(&prop.key, "enumerable") => {
                if enumerable || !is_true(&prop.value) {
                    return false;
                }
                enumerable = true;
            }
            Prop::KeyValue(prop) if prop_name_is(&prop.key, "get") => {
                let (false, Some(member)) = (get, getter) else {
                    return false;
                };
                if !is_member_getter(&prop.value, object, member) {
                    return false;
                }
                get = true;
            }
            _ => return false,
        }
    }
    enumerable && get == getter.is_some()
}

/// `function () { return object.member; }`
fn is_member_getter(expr: &Expr, object: &Atom, member: &str) -> bool {
    let Expr::Fn(getter) = strip_parens(expr) else {
        return false;
    };
    if !getter.function.params.is_empty() {
        return false;
    }
    let Some(body) = &getter.function.body else {
        return false;
    };
    let [Stmt::Return(ret)] = body.stmts.as_slice() else {
        return false;
    };
    let Some(Expr::Member(read)) = ret.arg.as_deref().map(strip_parens) else {
        return false;
    };
    matches!(strip_parens(&read.obj), Expr::Ident(read_object) if read_object.sym == *object)
        && member_name_is(&read.prop, member)
}

/// `true` or the minified `!0`.
fn is_true(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Lit(Lit::Bool(value)) => value.value,
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => {
            matches!(strip_parens(&unary.arg), Expr::Lit(Lit::Num(number)) if number.value == 0.0)
        }
        _ => false,
    }
}

/// `if (!guard)` test.
fn is_negated_guard(test: &Expr, input: &Atom, effects: &DecoratorEffects) -> bool {
    matches!(strip_parens(test), Expr::Unary(unary)
        if unary.op == UnaryOp::Bang && is_guard(&unary.arg, input, effects))
}

/// `input.webpackPolyfill`, or `object.children` on the input or the view.
fn is_guard(expr: &Expr, input: &Atom, effects: &DecoratorEffects) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    let Some((object, name)) = object_member(member) else {
        return false;
    };
    match name {
        "webpackPolyfill" => object == *input,
        "children" => object == *input || effects.view.as_ref() == Some(&object),
        _ => false,
    }
}

/// `Object.create(input)`
fn is_object_create_of(expr: &Expr, input: &Atom) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(callee) = strip_parens(callee) else {
        return false;
    };
    matches!(strip_parens(&callee.obj), Expr::Ident(object) if object.sym == "Object")
        && member_name_is(&callee.prop, "create")
        && matches!(call.args.as_slice(), [argument]
            if argument.spread.is_none()
                && matches!(strip_parens(&argument.expr), Expr::Ident(arg) if arg.sym == *input))
}

/// `object.member` for the decorator's own member names.
fn object_member(member: &MemberExpr) -> Option<(Atom, &'static str)> {
    let Expr::Ident(object) = strip_parens(&member.obj) else {
        return None;
    };
    let MemberProp::Ident(name) = &member.prop else {
        return None;
    };
    let name = ["children", "deprecate", "paths", "webpackPolyfill"]
        .into_iter()
        .find(|candidate| name.sym == *candidate)?;
    Some((object.sym.clone(), name))
}

fn member_name_is(prop: &MemberProp, name: &str) -> bool {
    matches!(prop, MemberProp::Ident(ident) if ident.sym == name)
}

fn prop_name_is(key: &PropName, name: &str) -> bool {
    match key {
        PropName::Ident(ident) => ident.sym == name,
        PropName::Str(value) => value.value.as_str() == Some(name),
        _ => false,
    }
}
