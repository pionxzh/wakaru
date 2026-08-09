//! CommonJS runtime facts that exist only inside a structurally extracted
//! webpack factory.
//!
//! Webpack initializes each factory's `module.exports` to an object and passes
//! that same object as `exports`. Normal single-file code does not carry this
//! proof, so these rewrites are enabled only by detector-owned module metadata
//! and never run for raw output.

use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BindingIdent, Callee, Decl,
    ExportDefaultExpr, Expr, Id, Ident, Lit, MemberExpr, MemberProp, Module, ModuleDecl,
    ModuleItem, Pat, SimpleAssignTarget, Stmt, UnaryOp, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;
use crate::rules::rename_utils::collect_module_names;
use crate::utils::paren::strip_parens;

pub(super) fn normalize_webpack_commonjs_runtime(
    module: &mut Module,
    unresolved_mark: Mark,
    enabled: bool,
) {
    if !enabled {
        return;
    }
    if !has_supported_module_shell(module) {
        return;
    }

    let mut functions = FunctionBindingCollector::default();
    module.visit_with(&mut functions);
    let uses = BindingUseIndex::collect(module);
    functions
        .bindings
        .retain(|binding| !uses.has_direct_write(binding));
    let capture = fresh_capture_ident(module);
    let mut candidate = module.clone();
    let mut normalizer = WebpackCommonJsRuntimeNormalizer {
        unresolved_mark,
        function_bindings: functions.bindings,
        capture: capture.clone(),
        matches: 0,
    };
    candidate.visit_mut_with(&mut normalizer);
    if normalizer.matches != 1 {
        return;
    }

    let mut runtime_references = RuntimeCommonJsReferenceFinder {
        unresolved_mark,
        found: false,
    };
    candidate.visit_with(&mut runtime_references);
    if runtime_references.found {
        return;
    }

    candidate
        .body
        .insert(0, capture_declaration(capture.clone()));
    candidate
        .body
        .push(ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(
            ExportDefaultExpr {
                span: DUMMY_SP,
                expr: Box::new(Expr::Ident(capture)),
            },
        )));
    *module = candidate;
}

fn has_supported_module_shell(module: &Module) -> bool {
    match module.body.as_slice() {
        [ModuleItem::Stmt(Stmt::Expr(_))] => true,
        [ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))), ModuleItem::Stmt(Stmt::Expr(_))] => {
            matches!(var.kind, VarDeclKind::Var | VarDeclKind::Let)
                && matches!(
                    var.decls.as_slice(),
                    [VarDeclarator {
                        name: Pat::Ident(_),
                        init: None,
                        ..
                    }]
                )
        }
        _ => false,
    }
}

fn fresh_capture_ident(module: &Module) -> Ident {
    let mut used_names = collect_module_names(module);
    let base: Atom = "_webpackDefault".into();
    let name = if used_names.insert(base.clone()) {
        base
    } else {
        (1usize..)
            .map(|suffix| Atom::from(format!("{base}_{suffix}")))
            .find(|candidate| used_names.insert(candidate.clone()))
            .expect("the generated name space is unbounded")
    };
    Ident::new(name, DUMMY_SP, SyntaxContext::empty())
}

fn capture_declaration(capture: Ident) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent::from(capture)),
            init: None,
            definite: false,
        }],
    }))))
}

struct RuntimeCommonJsReferenceFinder {
    unresolved_mark: Mark,
    found: bool,
}

impl Visit for RuntimeCommonJsReferenceFinder {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.ctxt.outer() == self.unresolved_mark
            && matches!(ident.sym.as_ref(), "module" | "exports" | "require")
        {
            self.found = true;
        }
    }
}

#[derive(Default)]
struct FunctionBindingCollector {
    bindings: HashSet<Id>,
}

impl Visit for FunctionBindingCollector {
    fn visit_fn_decl(&mut self, function: &swc_core::ecma::ast::FnDecl) {
        self.bindings
            .insert((function.ident.sym.clone(), function.ident.ctxt));
        function.function.visit_children_with(self);
    }
}

struct WebpackCommonJsRuntimeNormalizer {
    unresolved_mark: Mark,
    function_bindings: HashSet<Id>,
    capture: Ident,
    matches: usize,
}

impl VisitMut for WebpackCommonJsRuntimeNormalizer {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Some(replacement) = self.truthy_module_exports_branch(expr) {
            *expr = replacement;
            self.matches += 1;
            return;
        }
        if let Some(replacement) = self.non_undefined_factory_export(expr) {
            *expr = replacement;
            self.matches += 1;
        }
    }
}

impl WebpackCommonJsRuntimeNormalizer {
    /// Webpack has already initialized `module.exports`, so the CommonJS arm
    /// of `module.exports ? module.exports = value : browserFallback` is the
    /// only reachable arm inside an extracted factory.
    fn truthy_module_exports_branch(&self, expr: &Expr) -> Option<Expr> {
        let Expr::Cond(conditional) = strip_parens(expr) else {
            return None;
        };
        if !self.is_module_exports_expr(&conditional.test) {
            return None;
        }
        let assignment = self.module_exports_assignment(&conditional.cons)?;
        Some(self.capture_assignment(assignment.span, assignment.right.clone()))
    }

    /// Recover the minified UMD form
    ///
    /// ```text
    /// undefined === (result = factory.apply(exports, []))
    ///     || (module.exports = result)
    /// ```
    ///
    /// only when the immediately-created factory returns a stable function
    /// binding. That both proves the result is non-undefined and makes lifting
    /// the binding read out of the factory scope safe. Broader non-undefined
    /// expressions (especially arrows that capture the factory's `this`) fail
    /// closed. Replacing the native `apply` call relies on the project's
    /// `stable_builtins` assumption; the exact generated webpack/package shape
    /// is covered by a pinned producer fixture.
    fn non_undefined_factory_export(&self, expr: &Expr) -> Option<Expr> {
        let Expr::Bin(BinExpr {
            op: BinaryOp::LogicalOr,
            left,
            right,
            ..
        }) = strip_parens(expr)
        else {
            return None;
        };
        let export_assignment = self.module_exports_assignment(right)?;
        let Expr::Ident(exported_local) = strip_parens(&export_assignment.right) else {
            return None;
        };
        let value_assignment = self.undefined_checked_assignment(left)?;
        let AssignTarget::Simple(SimpleAssignTarget::Ident(value_target)) = &value_assignment.left
        else {
            return None;
        };
        if (value_target.id.sym.clone(), value_target.id.ctxt)
            != (exported_local.sym.clone(), exported_local.ctxt)
        {
            return None;
        }

        let returned = self.immediate_apply_return(&value_assignment.right)?;
        let mut value_assignment = value_assignment.clone();
        value_assignment.right = returned;
        Some(self.capture_assignment(
            export_assignment.span,
            Box::new(Expr::Assign(value_assignment)),
        ))
    }

    fn undefined_checked_assignment<'a>(&self, expr: &'a Expr) -> Option<&'a AssignExpr> {
        let Expr::Bin(BinExpr {
            op: BinaryOp::EqEq | BinaryOp::EqEqEq,
            left,
            right,
            ..
        }) = strip_parens(expr)
        else {
            return None;
        };
        match (self.is_undefined_expr(left), self.is_undefined_expr(right)) {
            (true, false) => self.simple_assignment(right),
            (false, true) => self.simple_assignment(left),
            _ => None,
        }
    }

    fn immediate_apply_return(&self, expr: &Expr) -> Option<Box<Expr>> {
        let Expr::Call(call) = strip_parens(expr) else {
            return None;
        };
        if call.args.len() != 2 || call.args.iter().any(|argument| argument.spread.is_some()) {
            return None;
        }
        let Expr::Ident(exports) = strip_parens(&call.args[0].expr) else {
            return None;
        };
        if exports.sym.as_ref() != "exports" || exports.ctxt.outer() != self.unresolved_mark {
            return None;
        }
        let Expr::Array(arguments) = strip_parens(&call.args[1].expr) else {
            return None;
        };
        if !arguments.elems.is_empty() {
            return None;
        }

        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee) else {
            return None;
        };
        if !matches!(prop, MemberProp::Ident(property) if property.sym.as_ref() == "apply") {
            return None;
        }
        let Expr::Fn(factory) = strip_parens(obj) else {
            return None;
        };
        if factory.function.is_async
            || factory.function.is_generator
            || !factory.function.params.is_empty()
        {
            return None;
        }
        let body = factory.function.body.as_ref()?;
        let [Stmt::Return(return_statement)] = body.stmts.as_slice() else {
            return None;
        };
        let returned = return_statement.arg.as_ref()?;
        let Expr::Ident(returned) = strip_parens(returned) else {
            return None;
        };
        self.function_bindings
            .contains(&(returned.sym.clone(), returned.ctxt))
            .then(|| Box::new(Expr::Ident(returned.clone())))
    }

    fn simple_assignment<'a>(&self, expr: &'a Expr) -> Option<&'a AssignExpr> {
        let Expr::Assign(assignment) = strip_parens(expr) else {
            return None;
        };
        (assignment.op == AssignOp::Assign).then_some(assignment)
    }

    fn module_exports_assignment<'a>(&self, expr: &'a Expr) -> Option<&'a AssignExpr> {
        let assignment = self.simple_assignment(expr)?;
        self.is_module_exports_target(&assignment.left)
            .then_some(assignment)
    }

    fn is_module_exports_expr(&self, expr: &Expr) -> bool {
        let Expr::Member(member) = strip_parens(expr) else {
            return false;
        };
        self.is_module_exports_member(member)
    }

    fn is_module_exports_target(&self, target: &AssignTarget) -> bool {
        matches!(target, AssignTarget::Simple(SimpleAssignTarget::Member(member)) if self.is_module_exports_member(member))
    }

    fn is_module_exports_member(&self, member: &MemberExpr) -> bool {
        let Expr::Ident(module) = member.obj.as_ref() else {
            return false;
        };
        module.sym.as_ref() == "module"
            && module.ctxt.outer() == self.unresolved_mark
            && matches!(&member.prop, MemberProp::Ident(property) if property.sym.as_ref() == "exports")
    }

    fn is_undefined_expr(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Unary(unary) if unary.op == UnaryOp::Void => {
                matches!(strip_parens(&unary.arg), Expr::Lit(Lit::Num(number)) if number.value == 0.0)
            }
            Expr::Ident(ident) => {
                ident.sym.as_ref() == "undefined" && ident.ctxt.outer() == self.unresolved_mark
            }
            _ => false,
        }
    }

    fn capture_assignment(&self, span: swc_core::common::Span, value: Box<Expr>) -> Expr {
        Expr::Assign(AssignExpr {
            span,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent::from(
                self.capture.clone(),
            ))),
            right: value,
        })
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, SourceMap, GLOBALS};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    use super::*;
    use crate::driver::io::{apply_fixer, parse_js, print_js};

    fn normalize(source: &str, enabled: bool) -> String {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module =
                parse_js(source, "fixture.js", cm.clone()).expect("fixture should parse");
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
            normalize_webpack_commonjs_runtime(&mut module, unresolved_mark, enabled);
            apply_fixer(&mut module).expect("fixture should fix");
            print_js(&module, cm).expect("fixture should print")
        })
    }

    #[test]
    fn initialized_runtime_selects_the_commonjs_umd_arm() {
        let input = r#"
!function(root) {
    function choose(value) { return value; }
    module.exports ? module.exports = choose : root.syntheticChoose = choose;
}(this);
"#;
        let output = normalize(input, true);
        assert!(output.contains("_webpackDefault = choose"), "{output}");
        assert!(
            output.contains("export default _webpackDefault"),
            "{output}"
        );
        assert!(!output.contains("module.exports"), "{output}");
        assert!(!output.contains("syntheticChoose"), "{output}");

        let ordinary = normalize(input, false);
        assert!(ordinary.contains("syntheticChoose"), "{ordinary}");
    }

    #[test]
    fn local_module_binding_does_not_use_the_webpack_runtime_fact() {
        let output = normalize(
            r#"
!function(module, root) {
    function choose(value) { return value; }
    module.exports ? module.exports = choose : root.syntheticChoose = choose;
}(localModule, this);
"#,
            true,
        );
        assert!(output.contains("syntheticChoose"), "{output}");
    }

    #[test]
    fn other_runtime_uses_or_multiple_candidates_fail_closed() {
        for input in [
            r#"
!function(root) {
    module.exports = null;
    function choose(value) { return value; }
    module.exports ? module.exports = choose : root.syntheticChoose = choose;
}(this);
"#,
            r#"
!function(root) {
    function choose(value) { return value; }
    module.exports ? module.exports = choose : root.syntheticChoose = choose;
    module.exports ? module.exports = choose : root.syntheticChooseAgain = choose;
}(this);
"#,
        ] {
            let output = normalize(input, true);
            assert!(output.contains("module.exports"), "{output}");
            assert!(output.contains("syntheticChoose"), "{output}");
            assert!(!output.contains("_webpackDefault"), "{output}");
        }
    }

    #[test]
    fn non_undefined_immediate_factory_recovers_the_commonjs_assignment() {
        let output = normalize(
            r#"
let result;
!function() {
    function createValue(value) { return { value: value }; }
    void 0 === (result = function() { return createValue; }.apply(exports, []))
        || (module.exports = result);
}();
"#,
            true,
        );
        assert!(
            output.contains("_webpackDefault = result = createValue"),
            "{output}"
        );
        assert!(
            output.contains("export default _webpackDefault"),
            "{output}"
        );
        assert!(!output.contains(".apply(exports"), "{output}");
    }

    #[test]
    fn effectful_or_unknown_factory_returns_fail_closed() {
        for factory in [
            "function() { observe(); return createValue; }",
            "function() { return maybeValue; }",
            "function() { return () => this; }",
        ] {
            let input = format!(
                r#"
!function() {{
    function createValue(value) {{ return {{ value: value }}; }}
    let result;
    void 0 === (result = ({factory}).apply(exports, []))
        || (module.exports = result);
}}();
"#
            );
            let output = normalize(&input, true);
            assert!(output.contains(".apply(exports"), "{output}");
        }
    }

    #[test]
    fn reassigned_function_return_fails_closed() {
        let output = normalize(
            r#"
!function() {
    function createValue(value) { return { value: value }; }
    createValue = maybeValue;
    let result;
    void 0 === (result = function() { return createValue; }.apply(exports, []))
        || (module.exports = result);
}();
"#,
            true,
        );
        assert!(output.contains(".apply(exports"), "{output}");
        assert!(output.contains("module.exports"), "{output}");
    }
}
