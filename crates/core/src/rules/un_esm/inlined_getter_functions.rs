//! Function expressions a minifier inlined into webpack's export getters.
//!
//! webpack exports a function declaration through a getter that returns it.
//! When the declaration has no other use, UglifyJS inlines it into that
//! getter (`producer webpack@5.111.1 production concatenateModules=false +
//! uglify-js@3.19.3`):
//!
//! ```text
//! // source
//! export function format(value) { return "<" + value + ">"; }
//! // webpack
//! r.d(t, { G: () => format }); function format(e) { return "<" + e + ">"; }
//! // then UglifyJS
//! r.d(t, { G: () => function (e) { return "<" + e + ">"; } });
//! ```
//!
//! [`hoist_inlined_getter_functions`] restores the declaration before the
//! `require.d` call and makes the getter return it, so the getter recovery
//! exports it like any other getter of a local binding. Each access of the
//! inlined getter created a new function; the declaration is one function,
//! as in the source (`inlined_export_getter_declaration` in
//! docs/rewrite-assumptions.md).

use crate::collections::HashSet;

use swc_core::ecma::ast::{ArrayLit, FnDecl, FnExpr, UpdateExpr};

use super::export_storage::identifier_base;
use super::*;

/// Move each function expression returned by an arrow getter of a top-level
/// `require.d(exports, …)` call into a function declaration before the call,
/// and make the getter return the declared binding. Returns whether any
/// getter changed.
///
/// The declaration takes the export name when no identifier in the module
/// is spelled that way, otherwise a fresh `_name`. A named function
/// expression's own name is renamed to it inside the body; one whose body
/// writes its own name stays inlined, because that write is a no-op on a
/// function expression's name and would rebind a declaration.
///
/// The getter must be the export's only definition: a module that uses
/// `exports` or `module.exports` other than as the target of a top-level
/// `require.d` or `require.r` call is left as is.
pub(super) fn hoist_inlined_getter_functions(module: &mut Module, unresolved_mark: Mark) -> bool {
    if !module.body.iter().any(|item| {
        export_definition_call(item, unresolved_mark).is_some_and(has_inlined_function_getter)
    }) || !exports_only_runtime_target(module, unresolved_mark)
    {
        return false;
    }
    let mut used_names = collect_all_identifier_names(module);
    let mut hoisted = false;
    let body = std::mem::take(&mut module.body);
    for mut item in body {
        let mut declarations = Vec::new();
        if let Some(call) = export_definition_call_mut(&mut item, unresolved_mark) {
            for (name, getter) in getter_slots_mut(call) {
                if let Some(declaration) = hoist_getter_function(&name, getter, &mut used_names) {
                    declarations.push(ModuleItem::Stmt(Stmt::Decl(Decl::Fn(declaration))));
                }
            }
        }
        hoisted |= !declarations.is_empty();
        module.body.extend(declarations);
        module.body.push(item);
    }
    hoisted
}

/// The top-level `require.d(exports, …)` call of `item`.
fn export_definition_call(item: &ModuleItem, unresolved_mark: Mark) -> Option<&CallExpr> {
    let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
        return None;
    };
    let Expr::Call(call) = &*statement.expr else {
        return None;
    };
    is_export_definition_call(call, unresolved_mark).then_some(call)
}

fn export_definition_call_mut(
    item: &mut ModuleItem,
    unresolved_mark: Mark,
) -> Option<&mut CallExpr> {
    let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
        return None;
    };
    let Expr::Call(call) = &mut *statement.expr else {
        return None;
    };
    is_export_definition_call(call, unresolved_mark).then_some(call)
}

fn is_export_definition_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    is_unresolved_member_expr(callee, "require", "d", unresolved_mark)
        && call.args.iter().all(|arg| arg.spread.is_none())
        && call.args.first().is_some_and(|target| {
            matches!(strip_parens(&target.expr), Expr::Ident(id)
                if is_unresolved_ident(id, "exports", unresolved_mark))
        })
}

/// Whether every reference to `exports` is the first argument of a top-level
/// `require.d(exports, …)` or `require.r(exports)` statement, and nothing
/// reads `module.exports`.
fn exports_only_runtime_target(module: &Module, unresolved_mark: Mark) -> bool {
    struct Counter {
        unresolved_mark: Mark,
        exports: usize,
        module_exports: bool,
    }
    impl Visit for Counter {
        fn visit_ident(&mut self, ident: &Ident) {
            if is_unresolved_ident(ident, "exports", self.unresolved_mark) {
                self.exports += 1;
            }
        }
        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if is_unresolved_member_expr_of(member, "module", "exports", self.unresolved_mark) {
                self.module_exports = true;
            }
            member.visit_children_with(self);
        }
    }
    let mut counter = Counter {
        unresolved_mark,
        exports: 0,
        module_exports: false,
    };
    module.visit_with(&mut counter);
    let targets = module
        .body
        .iter()
        .filter(|item| {
            let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
                return false;
            };
            let Expr::Call(call) = &*statement.expr else {
                return false;
            };
            let Callee::Expr(callee) = &call.callee else {
                return false;
            };
            (is_unresolved_member_expr(callee, "require", "d", unresolved_mark)
                || is_unresolved_member_expr(callee, "require", "r", unresolved_mark))
                && call.args.first().is_some_and(|target| {
                    target.spread.is_none()
                        && matches!(&*target.expr, Expr::Ident(id)
                            if is_unresolved_ident(id, "exports", unresolved_mark))
                })
        })
        .count();
    !counter.module_exports && counter.exports == targets
}

fn is_unresolved_member_expr_of(
    member: &MemberExpr,
    object: &str,
    property: &str,
    unresolved_mark: Mark,
) -> bool {
    matches!(strip_parens(&member.obj), Expr::Ident(id)
        if is_unresolved_ident(id, object, unresolved_mark))
        && static_member_name(&member.prop).is_some_and(|name| name == property)
}

/// Whether an argument after the target holds an inlined function getter, in
/// any of the forms [`getter_slots_mut`] reads. A value slot of the same
/// shape also counts; the rewrite skips it.
fn has_inlined_function_getter(call: &CallExpr) -> bool {
    call.args[1..].iter().any(|arg| match strip_parens(&arg.expr) {
        Expr::Object(object) => object.props.iter().any(|prop| {
            matches!(prop, PropOrSpread::Prop(prop)
                if matches!(&**prop, Prop::KeyValue(entry) if inlined_function(&entry.value).is_some()))
        }),
        Expr::Array(array) => array
            .elems
            .iter()
            .flatten()
            .any(|element| inlined_function(&element.expr).is_some()),
        getter => inlined_function(getter).is_some(),
    })
}

/// The getters of a `require.d(exports, …)` call that return an inlined
/// function, with their export names, in the forms
/// `lower_webpack_export_definitions` reads.
fn getter_slots_mut(call: &mut CallExpr) -> Vec<(Atom, &mut Box<Expr>)> {
    let mut slots = Vec::new();
    let [first, rest @ ..] = &mut call.args[1..] else {
        return slots;
    };
    match (&mut *first.expr, rest) {
        // `{ name: getter }`; rspack adds a second object of values.
        (Expr::Object(getters), [] | [_]) => {
            for prop in &mut getters.props {
                let PropOrSpread::Prop(prop) = prop else {
                    continue;
                };
                let Prop::KeyValue(KeyValueProp { key, value }) = &mut **prop else {
                    continue;
                };
                if let Some(name) = prop_name_as_atom(key) {
                    slots.push((name, value));
                }
            }
        }
        // webpack 5.108+: `[name, getter, name, 0, value, …]`.
        (Expr::Array(ArrayLit { elems, .. }), []) => {
            let mut elements = elems.iter_mut();
            while let Some(Some(key)) = elements.next() {
                let Expr::Lit(Lit::Str(key)) = &*key.expr else {
                    break;
                };
                let Some(name) = key.value.as_str().map(Atom::from) else {
                    break;
                };
                let Some(Some(binding)) = elements.next() else {
                    break;
                };
                if matches!(&*binding.expr, Expr::Lit(Lit::Num(num)) if num.value == 0.0) {
                    elements.next();
                    continue;
                }
                slots.push((name, &mut binding.expr));
            }
        }
        // webpack 4: `name, getter`.
        (Expr::Lit(Lit::Str(name)), [getter]) => {
            if let Some(name) = name.value.as_str().map(Atom::from) {
                slots.push((name, &mut getter.expr));
            }
        }
        _ => {}
    }
    slots.retain(|(_, getter)| inlined_function(getter).is_some());
    slots
}

/// The function expression an arrow getter `() => function … {}` returns.
/// An async or generator arrow returns something else.
fn inlined_function(getter: &Expr) -> Option<&FnExpr> {
    let Expr::Arrow(arrow) = getter else {
        return None;
    };
    if !arrow.params.is_empty() || arrow.is_async || arrow.is_generator {
        return None;
    }
    let ArrowFunctionBody::Expr(body) = &*arrow.body else {
        return None;
    };
    match strip_parens(body) {
        Expr::Fn(function) if !writes_own_name(function) => Some(function),
        _ => None,
    }
}

/// Whether the body of a named function expression assigns its own name.
fn writes_own_name(function: &FnExpr) -> bool {
    struct Finder {
        name: BindingId,
        found: bool,
    }
    impl Visit for Finder {
        fn visit_binding_ident(&mut self, ident: &BindingIdent) {
            self.found |= binding_id(&ident.id) == self.name;
        }
        fn visit_update_expr(&mut self, update: &UpdateExpr) {
            if matches!(strip_parens(&update.arg), Expr::Ident(ident) if binding_id(ident) == self.name)
            {
                self.found = true;
            }
            update.visit_children_with(self);
        }
    }
    let Some(ident) = &function.ident else {
        return false;
    };
    let mut finder = Finder {
        name: binding_id(ident),
        found: false,
    };
    function.function.visit_with(&mut finder);
    finder.found
}

fn hoist_getter_function(
    name: &Atom,
    getter: &mut Box<Expr>,
    used_names: &mut HashSet<Atom>,
) -> Option<FnDecl> {
    inlined_function(getter)?;
    let Expr::Arrow(arrow) = &mut **getter else {
        return None;
    };
    let ArrowFunctionBody::Expr(body) = &mut *arrow.body else {
        return None;
    };
    let local_name = if is_valid_identifier_name(name)
        && !is_reserved_binding_name(name)
        && name.as_ref() != "default"
        && used_names.insert(name.clone())
    {
        name.clone()
    } else {
        fresh_prefixed_name(&identifier_base(name), used_names)
    };
    let mut function = *std::mem::replace(body, Box::new(Expr::Invalid(Default::default())));
    while let Expr::Paren(paren) = function {
        function = *paren.expr;
    }
    let Expr::Fn(FnExpr {
        ident: own_name,
        mut function,
    }) = function
    else {
        unreachable!("inlined_function matched a function expression");
    };
    let local = match own_name {
        // The body's references to the expression's own name now refer to
        // the declaration, which binds the same function.
        Some(own_name) => {
            rename_bindings(
                &mut *function,
                &[BindingRename {
                    old: binding_id(&own_name),
                    new: local_name.clone(),
                }],
            );
            Ident::new(local_name, own_name.span, own_name.ctxt)
        }
        None => fresh_binding_ident(local_name, DUMMY_SP),
    };
    **body = Expr::Ident(local.clone());
    Some(FnDecl {
        ident: local,
        declare: false,
        function,
    })
}
