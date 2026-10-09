use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    CallExpr, Callee, Expr, Function, Ident, Lit, Module, SuperPropExpr, ThisExpr, WithStmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::utils::paren::strip_parens;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EvalCallSource {
    NoSource,
    Known(String),
    Unknown,
}

#[derive(Default)]
pub(crate) struct DirectEvalPresence {
    pub(crate) found: bool,
}

impl Visit for DirectEvalPresence {
    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        if direct_eval_call_source(call).is_some() {
            self.found = true;
            return;
        }
        call.visit_children_with(self);
    }
}

#[derive(Default)]
pub(crate) struct DirectEvalAnalyzer {
    pub(crate) known_direct_eval_sources: Vec<String>,
    pub(crate) known_indirect_eval_sources: Vec<String>,
    pub(crate) unknown_direct_eval: bool,
    pub(crate) unknown_indirect_eval: bool,
}

impl Visit for DirectEvalAnalyzer {
    fn visit_call_expr(&mut self, expr: &swc_core::ecma::ast::CallExpr) {
        if let Some(source) = direct_eval_call_source(expr) {
            match source {
                EvalCallSource::NoSource => {}
                EvalCallSource::Known(source) => self.known_direct_eval_sources.push(source),
                EvalCallSource::Unknown => self.unknown_direct_eval = true,
            }
            visit_eval_args(expr, self);
            return;
        }

        if let Some(source) = indirect_eval_call_source(expr) {
            match source {
                EvalCallSource::NoSource => {}
                EvalCallSource::Known(source) => self.known_indirect_eval_sources.push(source),
                EvalCallSource::Unknown => self.unknown_indirect_eval = true,
            }
            visit_eval_args(expr, self);
            return;
        }

        expr.visit_children_with(self);
    }
}

fn visit_eval_args(expr: &swc_core::ecma::ast::CallExpr, visitor: &mut impl Visit) {
    for arg in &expr.args {
        arg.expr.visit_with(visitor);
    }
}

pub(crate) fn direct_eval_call_source(
    expr: &swc_core::ecma::ast::CallExpr,
) -> Option<EvalCallSource> {
    if !is_direct_eval_call(expr) {
        return None;
    }
    Some(eval_call_source(expr))
}

fn indirect_eval_call_source(expr: &swc_core::ecma::ast::CallExpr) -> Option<EvalCallSource> {
    if !is_indirect_eval_call(expr) {
        return None;
    }
    Some(eval_call_source(expr))
}

fn eval_call_source(expr: &swc_core::ecma::ast::CallExpr) -> EvalCallSource {
    if expr.args.is_empty() {
        return EvalCallSource::NoSource;
    }

    if expr.args.iter().any(|arg| arg.spread.is_some()) {
        return EvalCallSource::Unknown;
    }

    expr.args
        .first()
        .and_then(|arg| eval_static_string(arg.expr.as_ref()))
        .map(EvalCallSource::Known)
        .unwrap_or(EvalCallSource::Unknown)
}

pub(crate) fn is_direct_eval_call(expr: &swc_core::ecma::ast::CallExpr) -> bool {
    let Callee::Expr(callee) = &expr.callee else {
        return false;
    };
    matches!(strip_parens(callee.as_ref()), Expr::Ident(id) if id.sym == "eval")
}

fn is_indirect_eval_call(expr: &swc_core::ecma::ast::CallExpr) -> bool {
    let Callee::Expr(callee) = &expr.callee else {
        return false;
    };
    match strip_parens(callee.as_ref()) {
        Expr::Seq(seq) => {
            matches!(seq.exprs.last().map(|expr| expr.as_ref()), Some(Expr::Ident(id)) if id.sym == "eval")
        }
        Expr::Call(call) => is_object_wrapped_eval_call(call),
        _ => false,
    }
}

fn is_object_wrapped_eval_call(call: &swc_core::ecma::ast::CallExpr) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !matches!(strip_parens(callee.as_ref()), Expr::Ident(id) if id.sym == "Object") {
        return false;
    }
    let Some(arg) = call.args.first() else {
        return false;
    };
    arg.spread.is_none()
        && matches!(strip_parens(arg.expr.as_ref()), Expr::Ident(id) if id.sym == "eval")
}

fn eval_static_string(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(Lit::Str(s)) => s.value.as_str().map(|value| value.to_string()),
        Expr::Call(call) => eval_hidden_require_string(call),
        _ => None,
    }
}

fn eval_hidden_require_string(call: &swc_core::ecma::ast::CallExpr) -> Option<String> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    let swc_core::ecma::ast::MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    if prop.sym != "replace" || call.args.len() != 2 {
        return None;
    }

    let Expr::Lit(Lit::Str(base)) = member.obj.as_ref() else {
        return None;
    };
    let Expr::Lit(Lit::Regex(regex)) = call.args[0].expr.as_ref() else {
        return None;
    };
    let Expr::Lit(Lit::Str(replacement)) = call.args[1].expr.as_ref() else {
        return None;
    };

    // Known generated-code shape for hiding CommonJS require from bundlers:
    // `eval("quire".replace(/^/, "re"))`. Keep this exact and avoid general
    // constant folding; `String.prototype.replace` can be monkey-patched.
    if base.value.as_str() == Some("quire")
        && regex.exp.as_ref() == "^"
        && replacement.value.as_str() == Some("re")
    {
        return Some("require".to_string());
    }

    None
}

/// Whether a known eval source only reads CommonJS `require`: `require`,
/// `require("x")`, or either followed by static member reads, optionally
/// ending in `;`. Generated code uses these shapes to hide a `require` call
/// from bundlers (`eval("require('crypto')")`, and the
/// `eval("quire".replace(/^/, "re"))` form `eval_static_string` resolves).
/// Such a source declares and assigns nothing.
pub(crate) fn is_require_read_source(source: &str) -> bool {
    let source = source.trim();
    let source = source.strip_suffix(';').unwrap_or(source);
    let Some(mut rest) = source.strip_prefix("require") else {
        return false;
    };
    if let Some(call) = rest.trim_start().strip_prefix('(') {
        let call = call.trim_start();
        let Some(quote) = call.chars().next().filter(|ch| matches!(ch, '"' | '\'')) else {
            return false;
        };
        let body = &call[1..];
        let Some(end) = body.find(quote) else {
            return false;
        };
        if body[..end].contains(['\\', '\n', '\r']) {
            return false;
        }
        let Some(after) = body[end + 1..].trim_start().strip_prefix(')') else {
            return false;
        };
        rest = after;
    }
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return true;
        }
        let Some(member) = rest.strip_prefix('.') else {
            return false;
        };
        let member = member.trim_start();
        if member.starts_with(|ch: char| ch.is_ascii_digit()) {
            return false;
        }
        let len = member
            .find(|ch: char| !(ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()))
            .unwrap_or(member.len());
        if len == 0 {
            return false;
        }
        rest = &member[len..];
    }
}

pub(crate) fn js_source_mentions_binding(source: &str, name: &Atom) -> bool {
    let name = name.as_ref();
    if name.is_empty() {
        return false;
    }

    let mut offset = 0;
    while let Some(index) = source[offset..].find(name) {
        let start = offset + index;
        let end = start + name.len();
        let before = source[..start].chars().next_back();
        let after = source[end..].chars().next();
        if !before.is_some_and(is_js_ident_part) && !after.is_some_and(is_js_ident_part) {
            return true;
        }
        offset = end;
    }

    false
}

fn is_js_ident_part(ch: char) -> bool {
    ch == '$' || ch == '_' || ch.is_ascii_alphanumeric()
}

/// Whether the module contains a `with` statement anywhere. Compilers never
/// emit one, so a module-wide check is the agreed stand-in for a with-scope
/// model: a rule that reads a free name as the global, renames a binding, or
/// introduces one skips the module instead (see the dynamic-scope section of
/// `docs/rewrite-assumptions.md`).
pub(crate) fn module_has_with_stmt(module: &Module) -> bool {
    struct Finder(bool);
    impl Visit for Finder {
        fn visit_with_stmt(&mut self, _: &WithStmt) {
            self.0 = true;
        }
    }
    let mut finder = Finder(false);
    module.visit_with(&mut finder);
    finder.0
}

/// Whether the module contains a `with` statement or a direct `eval` call
/// anywhere. This is the coarse module-wide skip from the dynamic-scope
/// policy for a rule that removes, renames, or introduces bindings and
/// cannot cheaply name every binding a known eval source could mention;
/// rules that can, use `DirectEvalAnalyzer` with `js_source_mentions_binding`
/// and keep the known-source best effort instead.
///
/// A direct eval whose known source only reads CommonJS `require`
/// (`is_require_read_source`) does not count while the module declares no
/// binding named `require`: no rule in this set can then rename or remove
/// the binding it reads, and SmartRename never names a binding `require`.
pub(crate) fn has_dynamic_scope_construct(module: &Module) -> bool {
    let mut finder = DynamicScopeConstructFinder::default();
    module.visit_with(&mut finder);
    finder.found
        || (finder.require_read_eval
            && super::rename_utils::module_declares_binding_named(module, "require"))
}

/// `has_dynamic_scope_construct` for part of a module, which cannot see
/// whether a binding the part reads is declared outside it, so every direct
/// eval counts.
pub(crate) fn node_has_dynamic_scope_construct<T>(node: &T) -> bool
where
    T: VisitWith<DynamicScopeConstructFinder> + ?Sized,
{
    let mut finder = DynamicScopeConstructFinder::default();
    node.visit_with(&mut finder);
    finder.found || finder.require_read_eval
}

#[derive(Default)]
pub(crate) struct DynamicScopeConstructFinder {
    found: bool,
    /// Saw a direct eval whose known source only reads `require`.
    require_read_eval: bool,
}

impl Visit for DynamicScopeConstructFinder {
    fn visit_with_stmt(&mut self, _: &WithStmt) {
        self.found = true;
    }

    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        if self.found {
            return;
        }
        if let Some(source) = direct_eval_call_source(call) {
            if matches!(&source, EvalCallSource::Known(source) if is_require_read_source(source)) {
                self.require_read_eval = true;
            } else {
                self.found = true;
            }
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_stmt(&mut self, stmt: &swc_core::ecma::ast::Stmt) {
        if !self.found {
            stmt.visit_children_with(self);
        }
    }
}

/// The module-wide skip from the dynamic-scope policy for a rule that
/// synthesizes a reference to the global `name` (`undefined`, `Infinity`).
/// The printed name has no static proof of resolving to the global once a
/// `with` statement or a direct `eval` can add bindings at runtime, so the
/// rule leaves the module untouched. A direct `eval` whose source is a known
/// string blocks only when that source mentions `name`, matching the
/// name-mention best effort binding-oriented rules use.
pub(crate) fn module_blocks_global_reference(module: &Module, name: &str) -> bool {
    if module_has_with_stmt(module) {
        return true;
    }
    let mut analyzer = DirectEvalAnalyzer::default();
    module.visit_with(&mut analyzer);
    analyzer.unknown_direct_eval
        || analyzer
            .known_direct_eval_sources
            .iter()
            .any(|source| js_source_mentions_binding(source, &Atom::from(name)))
}

/// The dynamic-scope skip for a rule that reads `undefined` as the global:
/// the module reads `undefined` by name, and a `with` statement or a direct
/// eval can rebind it (`module_blocks_global_reference`). A rule that also
/// accepts `void 0` keeps that form, and a module whose `undefined` is
/// spelled `void 0` throughout, as compilers emit it, keeps the rule.
pub(crate) fn module_reads_rebindable_undefined(module: &Module, unresolved_mark: Mark) -> bool {
    struct Finder {
        unresolved_mark: Mark,
        found: bool,
    }
    impl Visit for Finder {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == "undefined" && ident.ctxt.outer() == self.unresolved_mark {
                self.found = true;
            }
        }
    }
    if !has_dynamic_scope_construct(module) {
        return false;
    }
    let mut finder = Finder {
        unresolved_mark,
        found: false,
    };
    module.visit_with(&mut finder);
    finder.found && module_blocks_global_reference(module, "undefined")
}

/// Whether calling `function` can observe its receiver: it reads `this`,
/// or a direct eval may. Nested ordinary functions have their own receiver.
pub(crate) fn function_observes_receiver(function: &Function) -> bool {
    let mut analyzer = ReceiverSensitivityAnalyzer::default();
    function.params.visit_with(&mut analyzer);
    function.body.visit_with(&mut analyzer);
    analyzer.sensitive
}

#[derive(Default)]
struct ReceiverSensitivityAnalyzer {
    sensitive: bool,
}

impl Visit for ReceiverSensitivityAnalyzer {
    fn visit_this_expr(&mut self, _: &ThisExpr) {
        self.sensitive = true;
    }

    // `super.m()` calls `m` with this function's receiver, and `super.x`
    // passes it to an inherited getter.
    fn visit_super_prop_expr(&mut self, _: &SuperPropExpr) {
        self.sensitive = true;
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(source) = direct_eval_call_source(call) {
            let this_name: Atom = "this".into();
            self.sensitive |= match source {
                EvalCallSource::NoSource => false,
                EvalCallSource::Known(source) => js_source_mentions_binding(&source, &this_name),
                EvalCallSource::Unknown => true,
            };
            for argument in &call.args {
                argument.expr.visit_with(self);
            }
            return;
        }
        call.visit_children_with(self);
    }

    // Nested ordinary functions establish their own receiver. Arrows retain
    // the default traversal because they capture this function's receiver.
    fn visit_function(&mut self, _: &Function) {}
}

#[cfg(test)]
mod tests {
    use super::is_require_read_source;

    #[test]
    fn require_read_sources() {
        for source in [
            "require",
            "require('crypto')",
            "require(\"buffer\").Buffer",
            " require ( 'a' ) . b . c ; ",
            "require.resolve",
        ] {
            assert!(is_require_read_source(source), "{source}");
        }
    }

    #[test]
    fn other_sources_are_not_require_reads() {
        for source in [
            "",
            "requireX",
            "require(name)",
            "require('a', b)",
            "require('a')(b)",
            "require('a')[b]",
            "require('a'); x = 1",
            "require('a\\'b')",
            "require('a').0",
            "var require = 1",
            "require = f",
        ] {
            assert!(!is_require_read_source(source), "{source}");
        }
    }
}
