use swc_core::common::DUMMY_SP;
use swc_core::common::{Mark, Span};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, BinExpr, BinaryOp, CallExpr, Callee, Class, CondExpr,
    DoWhileStmt, Expr, ForInStmt, ForOfStmt, ForStmt, Function, IfStmt, ImportSpecifier, Lit,
    MemberProp, Module, ModuleDecl, ModuleItem, OptCall, OptChainBase, OptChainExpr, SwitchStmt,
    TryStmt, WhileStmt,
};
use swc_core::ecma::ast::{IdentName, Invalid, MemberExpr};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::facts::{ExportKind, ModuleFacts, ModuleFactsMap};

use crate::analysis::binding_id;
use crate::analysis::binding_uses::BindingId;
use crate::collections::HashSet;
use crate::rules::expr_utils::is_unresolved_ident;
use crate::utils::paren::strip_parens;

/// Where a `require` call that stays a call runs.
///
/// The classes follow the positions a source writes a require in: a load
/// that a bundler emitted for an `import` runs on every evaluation of the
/// module body, while a require in a function or a conditional position was
/// written by the source itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequirePosition {
    /// Runs on every evaluation of the module body.
    TopLevel,
    /// Inside a function, arrow, method, accessor, or class body (an IIFE
    /// included).
    Function,
    /// A branch of `if` or `?:`, the right side of `&&`, `||`, `??` or their
    /// assignments, an optional call or member, or a loop, `switch`, or `try`
    /// body.
    Conditional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeptRequire {
    pub span: Span,
    pub position: RequirePosition,
}

/// Every call of the free `require` in `module`, in visit order, except one
/// whose only argument is a numeric literal.
///
/// A numeric id names a module outside the input (unpack rewrites the ids it
/// resolves into relative paths), so the call fails in CommonJS as well as in
/// ESM: keeping such a module CommonJS would not make it run.
///
/// `module` must be resolved with `unresolved_mark`.
pub(crate) fn kept_require_calls(module: &Module, unresolved_mark: Mark) -> Vec<KeptRequire> {
    let mut finder = KeptRequireFinder {
        unresolved_mark,
        function_depth: 0,
        conditional_depth: 0,
        found: Vec::new(),
        blocking: false,
        imported_helpers: HashSet::default(),
    };
    module.visit_with(&mut finder);
    finder.found
}

/// [`kept_require_calls`] after `UnEsm` converted the module, without the
/// requires a later stage of unpack may still turn into imports from the
/// other modules' facts:
///
/// - the argument of a call of an imported helper, `helper(require("./p"))`
///   or `ns.helper(require("./p"))` (`cross_module_interop_import`);
/// - a source of a default-object composition shell,
///   `Object.assign(target, require("./p") || {})`
///   (`commonjs_default_object_composition`).
///
/// One the later stage leaves stays in ES module output.
pub(crate) fn blocking_require_calls(module: &Module, unresolved_mark: Mark) -> Vec<KeptRequire> {
    let mut imported = HashSet::default();
    for item in &module.body {
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
            imported.extend(import.specifiers.iter().map(|specifier| match specifier {
                ImportSpecifier::Named(named) => binding_id(&named.local),
                ImportSpecifier::Default(default) => binding_id(&default.local),
                ImportSpecifier::Namespace(namespace) => binding_id(&namespace.local),
            }));
        }
    }
    let mut finder = KeptRequireFinder {
        unresolved_mark,
        function_depth: 0,
        conditional_depth: 0,
        found: Vec::new(),
        blocking: true,
        imported_helpers: imported,
    };
    module.visit_with(&mut finder);
    finder.found
}

/// [`kept_require_calls`] for one node. The calls come in the order a
/// default `Visit` or `VisitMut` traversal of `node` meets them, so a
/// rewriting pass can pair them with the calls it visits.
pub(crate) fn kept_require_calls_in<N: VisitWith<KeptRequireFinder>>(
    node: &N,
    unresolved_mark: Mark,
) -> Vec<KeptRequire> {
    let mut finder = KeptRequireFinder {
        unresolved_mark,
        function_depth: 0,
        conditional_depth: 0,
        found: Vec::new(),
        blocking: false,
        imported_helpers: HashSet::default(),
    };
    node.visit_with(&mut finder);
    finder.found
}

/// Whether `call` calls the free `require` with anything but a numeric id.
pub(crate) fn is_kept_require_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !matches!(strip_parens(callee), Expr::Ident(id)
        if is_unresolved_ident(id, "require", unresolved_mark))
    {
        return false;
    }
    !matches!(call.args.as_slice(), [arg]
        if arg.spread.is_none() && matches!(strip_parens(&arg.expr), Expr::Lit(Lit::Num(_))))
}

pub(crate) struct KeptRequireFinder {
    unresolved_mark: Mark,
    function_depth: usize,
    conditional_depth: usize,
    found: Vec<KeptRequire>,
    /// Skip the requires a later stage converts ([`blocking_require_calls`]).
    blocking: bool,
    /// Import bindings whose call of a require is left for a later stage.
    imported_helpers: HashSet<BindingId>,
}

impl KeptRequireFinder {
    fn position(&self) -> RequirePosition {
        if self.function_depth > 0 {
            RequirePosition::Function
        } else if self.conditional_depth > 0 {
            RequirePosition::Conditional
        } else {
            RequirePosition::TopLevel
        }
    }

    fn calls_imported_helper(&self, call: &CallExpr) -> bool {
        if !self.blocking || self.imported_helpers.is_empty() {
            return false;
        }
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };
        let helper = match strip_parens(callee) {
            Expr::Ident(id) => id,
            Expr::Member(member) => match strip_parens(&member.obj) {
                Expr::Ident(id) => id,
                _ => return false,
            },
            _ => return false,
        };
        self.imported_helpers.contains(&binding_id(helper))
            && call.args.first().is_some_and(|first| {
                first.spread.is_none()
                    && matches!(&*first.expr, Expr::Call(inner)
                        if is_kept_require_call(inner, self.unresolved_mark))
            })
    }

    fn in_function<N: VisitWith<Self> + ?Sized>(&mut self, node: &N) {
        self.function_depth += 1;
        node.visit_children_with(self);
        self.function_depth -= 1;
    }

    fn conditional<N: VisitWith<Self> + ?Sized>(&mut self, node: &N) {
        self.conditional_depth += 1;
        node.visit_with(self);
        self.conditional_depth -= 1;
    }
}

impl Visit for KeptRequireFinder {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if is_kept_require_call(call, self.unresolved_mark) {
            self.found.push(KeptRequire {
                span: call.span,
                position: self.position(),
            });
        }
        if self.blocking && is_composition_call(call, self.unresolved_mark) {
            call.callee.visit_with(self);
            for (index, arg) in call.args.iter().enumerate() {
                match &*arg.expr {
                    Expr::Bin(BinExpr {
                        op: BinaryOp::LogicalOr,
                        left,
                        right,
                        ..
                    }) if index > 0 => {
                        if let Expr::Call(inner) = &**left {
                            inner.visit_children_with(self);
                        }
                        right.visit_with(self);
                    }
                    _ => arg.visit_with(self),
                }
            }
            return;
        }
        if self.calls_imported_helper(call) {
            // Skip the helper's require argument itself, not what it holds.
            call.callee.visit_with(self);
            for (index, arg) in call.args.iter().enumerate() {
                match &*arg.expr {
                    Expr::Call(inner) if index == 0 => inner.visit_children_with(self),
                    _ => arg.visit_with(self),
                }
            }
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_function(&mut self, function: &Function) {
        self.in_function(function);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.in_function(arrow);
    }

    // Field initializers, static blocks, and computed keys are counted with
    // the methods: a source that requires there wrote it itself.
    fn visit_class(&mut self, class: &Class) {
        self.in_function(class);
    }

    fn visit_if_stmt(&mut self, stmt: &IfStmt) {
        stmt.test.visit_with(self);
        self.conditional(&stmt.cons);
        if let Some(alt) = &stmt.alt {
            self.conditional(alt);
        }
    }

    fn visit_cond_expr(&mut self, expr: &CondExpr) {
        expr.test.visit_with(self);
        self.conditional(&expr.cons);
        self.conditional(&expr.alt);
    }

    fn visit_bin_expr(&mut self, expr: &BinExpr) {
        if matches!(
            expr.op,
            BinaryOp::LogicalAnd | BinaryOp::LogicalOr | BinaryOp::NullishCoalescing
        ) {
            expr.left.visit_with(self);
            self.conditional(&expr.right);
        } else {
            expr.visit_children_with(self);
        }
    }

    fn visit_assign_expr(&mut self, expr: &AssignExpr) {
        if matches!(
            expr.op,
            AssignOp::AndAssign | AssignOp::OrAssign | AssignOp::NullishAssign
        ) {
            expr.left.visit_with(self);
            self.conditional(&expr.right);
        } else {
            expr.visit_children_with(self);
        }
    }

    fn visit_opt_chain_expr(&mut self, expr: &OptChainExpr) {
        match &*expr.base {
            OptChainBase::Member(member) => {
                member.obj.visit_with(self);
                if let MemberProp::Computed(computed) = &member.prop {
                    self.conditional(computed);
                }
            }
            OptChainBase::Call(call) => self.visit_opt_call(call),
        }
    }

    fn visit_opt_call(&mut self, call: &OptCall) {
        call.callee.visit_with(self);
        self.conditional(&call.args);
    }

    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        stmt.init.visit_with(self);
        self.conditional(&stmt.test);
        self.conditional(&stmt.update);
        self.conditional(&stmt.body);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        self.conditional(&stmt.left);
        stmt.right.visit_with(self);
        self.conditional(&stmt.body);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        self.conditional(&stmt.left);
        stmt.right.visit_with(self);
        self.conditional(&stmt.body);
    }

    fn visit_while_stmt(&mut self, stmt: &WhileStmt) {
        self.conditional_depth += 1;
        stmt.visit_children_with(self);
        self.conditional_depth -= 1;
    }

    fn visit_do_while_stmt(&mut self, stmt: &DoWhileStmt) {
        self.conditional_depth += 1;
        stmt.visit_children_with(self);
        self.conditional_depth -= 1;
    }

    fn visit_switch_stmt(&mut self, stmt: &SwitchStmt) {
        stmt.discriminant.visit_with(self);
        self.conditional(&stmt.cases);
    }

    fn visit_try_stmt(&mut self, stmt: &TryStmt) {
        self.conditional_depth += 1;
        stmt.visit_children_with(self);
        self.conditional_depth -= 1;
    }
}

/// `Object.assign(target, require("./p") || {}, …)` with every source in
/// that form.
fn is_composition_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let is_assign = matches!(strip_parens(callee), Expr::Member(member)
        if matches!(strip_parens(&member.obj), Expr::Ident(object)
            if is_unresolved_ident(object, "Object", unresolved_mark))
            && member.prop.as_ident().is_some_and(|prop| prop.sym == "assign"));
    is_assign
        && call.args.len() >= 2
        && call.args[1..].iter().all(|arg| {
            arg.spread.is_none()
                && matches!(&*arg.expr, Expr::Bin(BinExpr {
                    op: BinaryOp::LogicalOr,
                    left,
                    right,
                    ..
                }) if matches!(&**left, Expr::Call(inner) if is_kept_require_call(inner, unresolved_mark))
                    && matches!(&**right, Expr::Object(object) if object.props.is_empty()))
        })
}

/// In a module that stays CommonJS, reads `.default` of each kept
/// `require("./p")` whose provider was `module.exports = v` and is now an
/// ES module with only `export default v`: `require` of an ES module returns
/// its namespace, and `v` is that namespace's `default`. Every consumer read
/// of the CommonJS value keeps its meaning, an interop helper's included.
pub(crate) fn run_kept_require_default_reads(
    module: &mut Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    unresolved_mark: Mark,
) {
    struct Rewriter<'a> {
        module_facts: &'a ModuleFactsMap,
        current_filename: Option<&'a str>,
        unresolved_mark: Mark,
    }

    impl VisitMut for Rewriter<'_> {
        fn visit_mut_expr(&mut self, expr: &mut Expr) {
            expr.visit_mut_children_with(self);
            let Expr::Call(call) = &*expr else {
                return;
            };
            if !is_kept_require_call(call, self.unresolved_mark) {
                return;
            }
            let [arg] = call.args.as_slice() else {
                return;
            };
            let Expr::Lit(Lit::Str(source)) = strip_parens(&arg.expr) else {
                return;
            };
            let Some(provider) = self
                .module_facts
                .get_from(self.current_filename, source.value.as_str().unwrap_or(""))
            else {
                return;
            };
            if !reads_value_as_default(provider) {
                return;
            }
            let call = std::mem::replace(expr, Expr::Invalid(Invalid { span: DUMMY_SP }));
            *expr = Expr::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(call),
                prop: MemberProp::Ident(IdentName::new("default".into(), DUMMY_SP)),
            });
        }
    }

    if module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
    {
        return;
    }
    module.visit_mut_with(&mut Rewriter {
        module_facts,
        current_filename,
        unresolved_mark,
    });
}

/// The provider's CommonJS value is the `default` of the namespace its ES
/// module output gives `require`.
fn reads_value_as_default(provider: &ModuleFacts) -> bool {
    provider.commonjs_value_is_default
        && !provider.stays_commonjs
        && !provider.has_export_all
        && provider.reexports.is_empty()
        && matches!(provider.exports.as_slice(), [export] if export.kind == ExportKind::Default)
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, FileName, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{parse_file_as_module, EsSyntax, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    use super::*;

    fn with_module<T>(source: &str, run: impl FnOnce(&Module, Mark) -> T) -> T {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(FileName::Anon.into(), source.to_string());
            let mut module = parse_file_as_module(
                &file,
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                None,
                &mut Vec::new(),
            )
            .expect("test source parses");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            run(&module, unresolved_mark)
        })
    }

    fn positions(source: &str) -> Vec<RequirePosition> {
        with_module(source, |module, mark| {
            kept_require_calls(module, mark)
                .into_iter()
                .map(|call| call.position)
                .collect()
        })
    }

    fn blocking(source: &str) -> usize {
        with_module(source, |module, mark| {
            blocking_require_calls(module, mark).len()
        })
    }

    use RequirePosition::{Conditional, Function, TopLevel};

    #[test]
    fn classifies_where_a_require_runs() {
        assert_eq!(
            positions(r#"require("a").b(); f(require("c"));"#),
            [TopLevel, TopLevel]
        );
        assert_eq!(
            positions(r#"function f() { require("a"); } var g = () => require("b");"#),
            [Function, Function]
        );
        assert_eq!(
            positions(r#"(function () { require("a"); })(); class C { m() { require("b"); } }"#),
            [Function, Function]
        );
        assert_eq!(
            positions(
                r#"var o = { get x() { return require("a"); }, set x(v) { require("b"); } };"#
            ),
            [Function, Function]
        );
        assert_eq!(
            positions(
                r#"if (require("t")) require("a"); else require("b");
var p = window.P ? window.P : require("c");
var q = window.Q || require("d");
o?.f(require("e"));"#
            ),
            [
                TopLevel,
                Conditional,
                Conditional,
                Conditional,
                Conditional,
                Conditional
            ]
        );
        assert_eq!(
            positions(
                r#"for (var i = require("a"); i; i++) require("b");
for (var k of require("c")) require("d");
switch (require("e")) { case 1: require("f"); }
try { require("g"); } catch (e) {}"#
            ),
            [
                TopLevel,
                Conditional,
                TopLevel,
                Conditional,
                TopLevel,
                Conditional,
                Conditional
            ]
        );
    }

    #[test]
    fn skips_numeric_ids_and_other_requires() {
        assert_eq!(
            positions(r#"require(123); require.e(1); r.bind(r, 2);"#),
            []
        );
        assert_eq!(
            positions(r#"require(name); require(`./${x}.json`);"#),
            [TopLevel, TopLevel]
        );
        assert_eq!(positions(r#"var require = load; require("a");"#), []);
    }

    #[test]
    fn imported_helper_and_composition_sources_do_not_block() {
        assert_eq!(
            blocking(
                r#"import h from "./h"; var a = h(require("./a")); var b = h.x(require("./b"));"#
            ),
            0
        );
        assert_eq!(
            blocking(
                r#"export default {}; Object.assign(o, require("./a") || {}, require("./b") || {});"#
            ),
            0
        );
        // A local helper is no import, and a composition needs every source
        // in its `|| {}` form.
        assert_eq!(
            blocking(
                r#"import h from "./h"; function g(x) { return x; } var a = g(require("./a"));"#
            ),
            1
        );
        assert_eq!(
            blocking(
                r#"export default {}; Object.assign(o, require("./a"), require("./b") || {});"#
            ),
            2
        );
    }
}
