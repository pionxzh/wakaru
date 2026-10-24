//! Member reads of a whole-value provider, read through one binding of the
//! provider's value.
//!
//! In unpack, barrier 0 proves which providers return from `require` the
//! value `v` of their one top-level `module.exports = v`
//! ([`crate::interface_plan`]). Its ES form is `export default v`, so
//! `require(p).name` reads `name` of the default export, whether or not `v`
//! has `name` and whether or not the provider ends ESM. The classification
//! below would otherwise make `import { name } from "p"`, which fails to
//! link against a default-only module.
//!
//! A provider may also add a name to `v` with a top-level
//! `module.exports.name = e`, which `UnEsm` exports by name, not on the
//! default. A read of such a name keeps the ordinary classification, a named
//! import, and so does an object pattern that reads one, has a rest element,
//! or has a computed key, as a whole.
//!
//! The pass binds the provider's value once and reads the member from that
//! binding where the input read it:
//!
//! ```text
//! before(); var map = require("./p").map; f(a(), require("./p").filter);
//! // becomes
//! before(); var _p = require("./p"); var map = _p.map; f(a(), _p.filter);
//! ```
//!
//! The ordinary require conversion then turns the binding into
//! `import _p from "./p"`. Only the provider's evaluation moves, as it does
//! for every require that becomes an import; the member read keeps its
//! place. The reads it takes are the ones that sit at the top level: a
//! declarator's member chain (`var x = require(p).a.b`), any object pattern
//! destructuring `require(p)` (defaults and rest included), a direct argument
//! of a top-level call, and the right side of a top-level assignment
//! (`exports.x = require(p).y`). A `.default` read is left to the
//! default-member classification. A read inside a function, a condition, or
//! any other expression keeps its `require`, as it would without the plan.
//!
//! A direct eval in the consumer does not stop the pass: the rest of `UnEsm`
//! turns the consumer's requires into imports either way. The fresh binding
//! avoids the names the eval's known code mentions, and under unknown code
//! takes a `_`-prefixed name rather than the specifier's stem.
//!
//! A consumer's require of a CommonJS passthrough is rewritten to the module
//! behind it before this pass runs (`InterfacePlan::passthrough_sources`), so
//! a member read through a passthrough of a whole-value provider is read the
//! same way.

use swc_core::ecma::ast::ObjectPat;

use super::*;

/// Rewrites the member reads of every source in `sources` as described in
/// the module docs. Returns, for each `require()` call it replaced, the
/// binding that now holds its value and the call's span, so the import tag
/// of that binding lists every call it stands for.
pub(super) fn read_whole_value_members(
    module: &mut Module,
    unresolved_mark: Mark,
    sources: &HashMap<String, Vec<Atom>>,
) -> Vec<(BindingId, Span)> {
    if sources.is_empty() {
        return Vec::new();
    }
    // The first read of each source: its item, and a name hint.
    let mut first_reads: Vec<(String, usize, Option<Atom>)> = Vec::new();
    for (index, item) in module.body.iter_mut().enumerate() {
        visit_read_slots(item, unresolved_mark, &mut |slot, hint, names| {
            let Some(source) = slot_source(slot, unresolved_mark) else {
                return;
            };
            if reads_through_default(sources, &source, names)
                && !first_reads.iter().any(|(s, ..)| *s == source)
            {
                first_reads.push((source, index, hint));
            }
        });
    }
    if first_reads.is_empty() {
        return Vec::new();
    }
    let mut eval_analyzer = DirectEvalAnalyzer::default();
    module.visit_with(&mut eval_analyzer);

    let uses = BindingUseIndex::collect(module);
    let mut used_names = collect_all_identifier_names(module);
    // For each source: its binding, and the item before which a fresh one is
    // declared.
    let mut bindings: HashMap<String, Ident> = HashMap::default();
    let mut fresh: HashMap<usize, Vec<String>> = HashMap::default();
    for (source, index, hint) in &first_reads {
        let earlier =
            earlier_require_binding(&module.body[..*index], source, &uses, unresolved_mark);
        let binding = match earlier {
            Some(existing) => existing,
            None => {
                let name = binding_name(source, hint.as_ref(), &mut used_names, &eval_analyzer);
                fresh.entry(*index).or_default().push(source.clone());
                fresh_binding_ident(name, DUMMY_SP)
            }
        };
        bindings.insert(source.clone(), binding);
    }

    let mut replaced: Vec<(BindingId, Span)> = Vec::new();
    let mut declared: HashSet<String> = HashSet::default();
    let body = std::mem::take(&mut module.body);
    for (index, mut item) in body.into_iter().enumerate() {
        let mut calls: Vec<(String, CallExpr)> = Vec::new();
        visit_read_slots(&mut item, unresolved_mark, &mut |slot, _, names| {
            let Some(source) = slot_source(slot, unresolved_mark) else {
                return;
            };
            if !reads_through_default(sources, &source, names) {
                return;
            }
            let Some(binding) = bindings.get(&source) else {
                return;
            };
            let call = std::mem::replace(slot, Box::new(Expr::Ident(binding.clone())));
            let Expr::Call(call) = *strip_parens_owned(call) else {
                unreachable!("slot_source matched a require call");
            };
            replaced.push((binding_id(binding), call.span));
            calls.push((source, call));
        });
        // A fresh binding holds the first call it replaces, which keeps that
        // call's span for the import tag.
        for source in fresh.get(&index).into_iter().flatten() {
            if !declared.insert(source.clone()) {
                continue;
            }
            let Some((_, call)) = calls.iter().find(|(called, _)| called == source) else {
                continue;
            };
            module
                .body
                .push(require_binding_item(bindings[source].clone(), call.clone()));
        }
        module.body.push(item);
    }
    replaced
}

/// `var L = require(source)` among `items`, a single declarator whose
/// binding is declared once and never written: it already holds the value.
fn earlier_require_binding(
    items: &[ModuleItem],
    source: &str,
    uses: &BindingUseIndex,
    unresolved_mark: Mark,
) -> Option<Ident> {
    items.iter().rev().find_map(|item| {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            return None;
        };
        let [declarator] = var.decls.as_slice() else {
            return None;
        };
        let Pat::Ident(binding) = &declarator.name else {
            return None;
        };
        let Expr::Call(call) = declarator.init.as_deref()? else {
            return None;
        };
        let id = binding_id(&binding.id);
        (is_require_call(call, unresolved_mark).as_deref() == Some(source)
            && uses.has_single_declaration(&id)
            && !uses.has_direct_write(&id))
        .then(|| binding.id.clone())
    })
}

/// The name of a fresh binding of `source`'s value: the specifier's file
/// stem when it is a free identifier, else `_` and the hint of the first
/// read (`_x` for `var x = require(p).name`). Under a direct eval of unknown code,
/// a plain stem (`utils`) is a likelier name for that code to read than a
/// `_`-prefixed one, so the stem is not used.
fn binding_name(
    source: &str,
    hint: Option<&Atom>,
    used_names: &mut HashSet<Atom>,
    eval_analyzer: &DirectEvalAnalyzer,
) -> Atom {
    if let Some(stem) = specifier_basename(source).filter(|stem| {
        !eval_analyzer.unknown_direct_eval
            && !used_names.contains(stem)
            && !direct_eval_mentions_name(eval_analyzer, stem)
    }) {
        used_names.insert(stem.clone());
        return stem;
    }
    let base = hint
        .filter(|hint| is_valid_identifier_name(hint))
        .cloned()
        .unwrap_or_else(|| Atom::from("default"));
    loop {
        let name = fresh_prefixed_name(&base, used_names);
        if !direct_eval_mentions_name(eval_analyzer, &name) {
            return name;
        }
    }
}

fn require_binding_item(local: Ident, call: CallExpr) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: local,
                type_ann: None,
            }),
            init: Some(Box::new(Expr::Call(call))),
            definite: false,
        }],
    }))))
}

/// The names a slot reads of the value: `None` for a pattern that may read
/// any name (a rest element or a computed key).
type ReadNames<'a> = Option<&'a [Atom]>;

/// Whether the slot of `source` reading `names` reads through the default
/// import: `source` is a whole-value source, and the slot reads no name its
/// module adds with `module.exports.name = e`, which `UnEsm` exports by
/// name. A slot that touches an added name is left to the ordinary
/// classification as a whole.
fn reads_through_default(
    sources: &HashMap<String, Vec<Atom>>,
    source: &str,
    names: ReadNames<'_>,
) -> bool {
    let Some(added) = sources.get(source) else {
        return false;
    };
    added.is_empty() || names.is_some_and(|names| !names.iter().any(|name| added.contains(name)))
}

/// The source of the `require(source)` call a slot holds.
fn slot_source(slot: &Expr, unresolved_mark: Mark) -> Option<String> {
    match strip_parens(slot) {
        Expr::Call(call) => is_require_call(call, unresolved_mark),
        _ => None,
    }
}

fn strip_parens_owned(expr: Box<Expr>) -> Box<Expr> {
    match *expr {
        Expr::Paren(paren) => strip_parens_owned(paren.expr),
        expr => Box::new(expr),
    }
}

/// Calls `f` with each slot of `item` that holds a `require()` call whose
/// value is read at the top level, with a name hint (the declarator's
/// binding, the first key of the pattern destructuring the value, or the
/// member read on it) and the names the slot reads.
fn visit_read_slots(
    item: &mut ModuleItem,
    unresolved_mark: Mark,
    f: &mut impl FnMut(&mut Box<Expr>, Option<Atom>, ReadNames<'_>),
) {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
            for declarator in &mut var.decls {
                let Some(init) = declarator.init.as_mut() else {
                    continue;
                };
                match &declarator.name {
                    Pat::Object(pattern) => {
                        if slot_source(init, unresolved_mark).is_some() {
                            let names = pattern_names(pattern);
                            f(init, first_pattern_key(pattern), names.as_deref());
                            continue;
                        }
                    }
                    Pat::Ident(binding) => {
                        let local = binding.id.sym.clone();
                        if visit_member_chain_slot(init, unresolved_mark, &mut |slot, _, names| {
                            f(slot, Some(local.clone()), names)
                        }) {
                            continue;
                        }
                    }
                    _ => continue,
                }
                visit_call_arg_slots(init, unresolved_mark, f);
            }
        }
        ModuleItem::Stmt(Stmt::Expr(statement)) => match statement.expr.as_mut() {
            Expr::Assign(assign) if assign.op == AssignOp::Assign => {
                visit_member_chain_slot(&mut assign.right, unresolved_mark, f);
            }
            expr => visit_call_arg_slots(expr, unresolved_mark, f),
        },
        _ => {}
    }
}

/// `require(p).a.b`: the slot of the call, when the member read on it is
/// static and not `default`. Returns whether `expr` was such a chain.
fn visit_member_chain_slot(
    expr: &mut Box<Expr>,
    unresolved_mark: Mark,
    f: &mut impl FnMut(&mut Box<Expr>, Option<Atom>, ReadNames<'_>),
) -> bool {
    let Expr::Member(member) = expr.as_mut() else {
        return false;
    };
    if slot_source(&member.obj, unresolved_mark).is_some() {
        let Some(name) = static_member_name(&member.prop) else {
            return false;
        };
        if name == "default" {
            return false;
        }
        f(
            &mut member.obj,
            Some(name.clone()),
            Some(std::slice::from_ref(&name)),
        );
        return true;
    }
    visit_member_chain_slot(&mut member.obj, unresolved_mark, f)
}

/// The direct arguments `require(p).name` of a call that is the whole
/// expression.
fn visit_call_arg_slots(
    expr: &mut Expr,
    unresolved_mark: Mark,
    f: &mut impl FnMut(&mut Box<Expr>, Option<Atom>, ReadNames<'_>),
) {
    let Expr::Call(call) = expr else {
        return;
    };
    if !matches!(call.callee, Callee::Expr(_)) || is_require_call(call, unresolved_mark).is_some() {
        return;
    }
    for arg in &mut call.args {
        if arg.spread.is_none() && matches!(arg.expr.as_ref(), Expr::Member(_)) {
            let Expr::Member(member) = arg.expr.as_mut() else {
                continue;
            };
            if slot_source(&member.obj, unresolved_mark).is_none() {
                continue;
            }
            let Some(name) = static_member_name(&member.prop).filter(|name| name != "default")
            else {
                continue;
            };
            f(
                &mut member.obj,
                Some(name.clone()),
                Some(std::slice::from_ref(&name)),
            );
        }
    }
}

/// The keys an object pattern reads, or `None` when it has a rest element
/// or a computed key.
fn pattern_names(pattern: &ObjectPat) -> Option<Vec<Atom>> {
    pattern
        .props
        .iter()
        .map(|prop| match prop {
            ObjectPatProp::KeyValue(prop) => match &prop.key {
                PropName::Ident(key) => Some(key.sym.clone()),
                PropName::Str(key) => key.value.as_str().map(Atom::from),
                _ => None,
            },
            ObjectPatProp::Assign(prop) => Some(prop.key.id.sym.clone()),
            ObjectPatProp::Rest(_) => None,
        })
        .collect()
}

fn first_pattern_key(pattern: &ObjectPat) -> Option<Atom> {
    match pattern.props.first()? {
        ObjectPatProp::KeyValue(prop) => match &prop.key {
            PropName::Ident(key) => Some(key.sym.clone()),
            PropName::Str(key) => key.value.as_str().map(Atom::from),
            _ => None,
        },
        ObjectPatProp::Assign(prop) => Some(prop.key.id.sym.clone()),
        ObjectPatProp::Rest(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, SourceMap, GLOBALS};
    use swc_core::ecma::transforms::base::resolver;

    use super::*;

    /// `source` after the pass, with `./p` a whole-value source.
    fn read(source: &str) -> String {
        read_adding(source, &[])
    }

    /// `source` after the pass, with `./p` a whole-value source whose module
    /// adds `added` to its value.
    fn read_adding(source: &str, added: &[&str]) -> String {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module =
                crate::unpacker::parse_es_module(source, "consumer.js", cm.clone()).unwrap();
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            let added = added.iter().map(|name| Atom::from(*name)).collect();
            let sources = HashMap::from_iter([("./p".to_string(), added)]);
            read_whole_value_members(&mut module, unresolved_mark, &sources);
            crate::unpacker::emit_esm::emit_module_raw(&module, cm).unwrap()
        })
    }

    #[test]
    fn a_consumer_with_a_direct_eval_still_reads_through_the_default() {
        // Unknown code: a `_` name rather than the stem.
        assert_eq!(
            read(
                r#"var { x } = require("./p");
eval(code);
"#
            ),
            r#"var _x = require("./p");
var { x } = _x;
eval(code);
"#
        );
        // Known code takes the stem unless it mentions it.
        assert_eq!(
            read(
                r#"var x = require("./p").x;
eval("probe()");
"#
            ),
            r#"var p = require("./p");
var x = p.x;
eval("probe()");
"#
        );
        assert_eq!(
            read(
                r#"var x = require("./p").x;
eval("p(_x)");
"#
            ),
            r#"var _x_2 = require("./p");
var x = _x_2.x;
eval("p(_x)");
"#
        );
    }

    #[test]
    fn a_read_of_an_added_name_keeps_its_require() {
        // `UnEsm` exports `beta` by name: its read stays a named import.
        assert_eq!(
            read_adding(
                r#"var alpha = require("./p").alpha;
var beta = require("./p").beta;
use(require("./p").beta, require("./p").gamma);
x = require("./p").beta.delta;
"#,
                &["beta"]
            ),
            r#"var p = require("./p");
var alpha = p.alpha;
var beta = require("./p").beta;
use(require("./p").beta, p.gamma);
x = require("./p").beta.delta;
"#
        );
    }

    #[test]
    fn a_pattern_that_may_read_an_added_name_keeps_its_require() {
        // A pattern reads through the default only when every key it reads
        // is known and none is added.
        let source = r#"var { alpha, beta } = require("./p");
var { gamma, ...rest } = require("./p");
var { [key]: delta } = require("./p");
"#;
        assert_eq!(read_adding(source, &["beta"]), source);
        assert_eq!(
            read_adding(
                r#"var { alpha, "gamma": g = 1 } = require("./p");"#,
                &["beta"]
            ),
            r#"var p = require("./p");
var { alpha, "gamma": g = 1 } = p;
"#
        );
        // Without added names, any pattern reads through the default.
        assert_eq!(
            read(r#"var { gamma, ...rest } = require("./p");"#),
            r#"var p = require("./p");
var { gamma, ...rest } = p;
"#
        );
    }

    #[test]
    fn reads_outside_the_top_level_member_positions_keep_their_require() {
        let source = r#"var a = require("./p").default;
var b = require("./p")[key];
var c = cond && require("./p").c;
var d = require("./p")["default"].d;
function f() {
    return require("./p").f;
}
use(require("./p").default, require("./p")[key], require("./p"));
var e = require("./q").e;
"#;
        assert_eq!(read(source), source);
    }

    #[test]
    fn an_earlier_stable_binding_of_the_value_is_reused() {
        assert_eq!(
            read(
                r#"var p = require("./p");
var m = require("./p").m;
x = require("./p").n.o;
"#
            ),
            r#"var p = require("./p");
var m = p.m;
x = p.n.o;
"#
        );
        // A binding the module writes is not the value any more.
        assert_eq!(
            read(
                r#"var p = require("./p");
p = other;
var m = require("./p").m;
"#
            ),
            r#"var p = require("./p");
p = other;
var _m = require("./p");
var m = _m.m;
"#
        );
    }
}
