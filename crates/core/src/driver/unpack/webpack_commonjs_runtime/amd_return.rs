//! Collapse generated AMD return factories, then account for the remaining
//! initialization-time CommonJS slot without moving the surrounding code.

use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::*;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::{
    empty_object_declaration, fresh_capture_ident, module_exports_assignment,
    FactoryInvocationObservations, RuntimeCommonJsReferenceFinder,
    WebpackCommonJsRuntimeNormalizer,
};
use crate::collections::HashSet;
use crate::rules::eval_utils::{module_has_with_stmt, DirectEvalAnalyzer};
use crate::utils::paren::strip_parens;

pub(super) fn restore(module: &mut Module, unresolved_mark: Mark) -> bool {
    // Most extracted modules have no AMD return factory. Avoid cloning their
    // ASTs or collecting names/dynamic-scope evidence for this narrow path.
    let mut finder = FactoryFinder {
        unresolved_mark,
        found: false,
    };
    module.visit_with(&mut finder);
    if !finder.found {
        return false;
    }
    if module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
        || module_has_with_stmt(module)
    {
        return false;
    }
    let mut eval = DirectEvalAnalyzer::default();
    module.visit_with(&mut eval);
    if eval.unknown_direct_eval || !eval.known_direct_eval_sources.is_empty() {
        return false;
    }
    let capture = fresh_capture_ident(module);
    let mut candidate = module.clone();
    let mut recovery = Recovery {
        parser: WebpackCommonJsRuntimeNormalizer {
            unresolved_mark,
            function_bindings: HashSet::default(),
            capture: capture.clone(),
            matches: 0,
        },
        factories: 0,
        writes: 0,
        invalid: false,
    };
    candidate.visit_mut_with(&mut recovery);
    if recovery.invalid || recovery.factories == 0 || recovery.writes == 0 {
        return false;
    }
    // Only known immediately invoked bodies were visited. References in an
    // escaping/deferred function, module aliases, or other runtime operations
    // reject the entire candidate, including its speculative factory edits.
    let mut runtime = RuntimeCommonJsReferenceFinder {
        unresolved_mark,
        found: false,
    };
    candidate.visit_with(&mut runtime);
    let mut observations = FactoryInvocationObservations::default();
    candidate.visit_with(&mut observations);
    if runtime.found || observations.found {
        return false;
    }
    candidate
        .body
        .insert(0, empty_object_declaration(capture.clone()));
    // Produce ordinary CJS for the existing UnEsm pipeline. Do not introduce
    // a second module-format policy or hoist conditional assignments.
    candidate
        .body
        .push(module_exports_assignment(capture, unresolved_mark));
    *module = candidate;
    true
}

struct Recovery {
    parser: WebpackCommonJsRuntimeNormalizer,
    factories: usize,
    writes: usize,
    invalid: bool,
}

impl Recovery {
    fn module_null_comparison(&self, binary: &BinExpr) -> Option<bool> {
        let is_null = |expr: &Expr| matches!(strip_parens(expr), Expr::Lit(Lit::Null(_)));
        if !(runtime_ident(&binary.left, "module", self.parser.unresolved_mark)
            && is_null(&binary.right)
            || is_null(&binary.left)
                && runtime_ident(&binary.right, "module", self.parser.unresolved_mark))
        {
            return None;
        }
        match binary.op {
            BinaryOp::EqEq | BinaryOp::EqEqEq => Some(false),
            BinaryOp::NotEq | BinaryOp::NotEqEq => Some(true),
            _ => None,
        }
    }

    fn immediate_function(&mut self, function: &mut FnExpr) {
        if function.ident.is_some() || function.function.is_async || function.function.is_generator
        {
            return;
        }
        let Some(body) = &mut function.function.body else {
            return;
        };
        // Required even when the invocation stays unchanged: the recovered
        // output becomes a strict ES module, so a sloppy `this` that read the
        // global object would read undefined instead.
        let mut observations = FactoryInvocationObservations::default();
        function.function.params.visit_with(&mut observations);
        body.visit_with(&mut observations);
        if observations.found {
            self.invalid = true;
            return;
        }
        body.visit_mut_with(self);
    }
}

impl VisitMut for Recovery {
    fn visit_mut_function(&mut self, _: &mut Function) {}
    fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}
    fn visit_mut_class(&mut self, _: &mut Class) {}

    fn visit_mut_assign_expr(&mut self, assignment: &mut AssignExpr) {
        // Do not rewrite property targets/compound assignments through a
        // different receiver. Unhandled runtime targets fail the final scan.
        if assignment.op == AssignOp::Assign
            && self.parser.is_module_exports_target(&assignment.left)
        {
            assignment.left = AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent::from(
                self.parser.capture.clone(),
            )));
            self.writes += 1;
        }
        assignment.right.visit_mut_with(self);
    }

    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        if let Expr::Call(call) = expression {
            if let Some(value) = return_factory(call, self.parser.unresolved_mark) {
                *expression = Expr::Ident(value);
                self.factories += 1;
                return;
            }
            if let Callee::Expr(callee) = &mut call.callee {
                // Replacing this callee would change its receiver from module
                // to undefined. Leave it for the final runtime rejection.
                if self.parser.is_module_exports_expr(callee) {
                    return;
                }
                let mut direct = None;
                if let Expr::Member(member) = strip_parens(callee) {
                    if matches!(&member.prop, MemberProp::Ident(name) if name.sym == "call")
                        && matches!(call.args.as_slice(), [arg] if arg.spread.is_none()
                            && matches!(strip_parens(&arg.expr), Expr::This(_)))
                    {
                        if let Expr::Fn(function) = strip_parens(&member.obj) {
                            if function.ident.is_none()
                                && !function.function.is_async
                                && !function.function.is_generator
                                && function.function.params.is_empty()
                            {
                                direct = Some(function.clone());
                            }
                        }
                    }
                }
                if let Some(mut function) = direct {
                    self.immediate_function(&mut function);
                    **callee = Expr::Fn(function);
                    call.args.clear();
                    return;
                }
                if let Expr::Fn(function) = crate::utils::paren::strip_parens_mut(callee) {
                    self.immediate_function(function);
                    call.args.visit_mut_with(self);
                    return;
                }
            }
        }
        // These contexts can observe/delete the member reference rather than
        // merely its value. They are outside this slot-localization proof.
        if matches!(
            expression,
            Expr::TaggedTpl(_)
                | Expr::OptChain(_)
                | Expr::Unary(UnaryExpr {
                    op: UnaryOp::Delete,
                    ..
                })
        ) {
            return;
        }
        if self.parser.is_module_exports_expr(expression) {
            *expression = Expr::Ident(self.parser.capture.clone());
            return;
        }
        if let Expr::Bin(binary) = expression {
            if let Some(value) = self.module_null_comparison(binary) {
                *expression = Expr::Lit(Lit::Bool(Bool {
                    span: DUMMY_SP,
                    value,
                }));
                return;
            }
        }
        expression.visit_mut_children_with(self);
    }
}

struct FactoryFinder {
    unresolved_mark: Mark,
    found: bool,
}

impl Visit for FactoryFinder {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if !self.found && return_factory(call, self.unresolved_mark).is_some() {
            self.found = true;
        }
        if !self.found {
            call.visit_children_with(self);
        }
    }
}

fn runtime_ident(expression: &Expr, name: &str, unresolved_mark: Mark) -> bool {
    matches!(strip_parens(expression), Expr::Ident(id)
        if id.sym == name && id.ctxt.outer() == unresolved_mark)
}

// Eliminating the native call/apply uses the existing stable_builtins
// assumption. The fresh webpack module's exports slot is a data property;
// rejecting every module-object escape/rebinding preserves that fact here.
fn return_factory(call: &CallExpr, unresolved_mark: Mark) -> Option<Ident> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    let Expr::Fn(factory) = strip_parens(&member.obj) else {
        return None;
    };
    if factory.ident.is_some()
        || factory.function.is_async
        || factory.function.is_generator
        || !factory.function.params.is_empty()
        || call.args.iter().any(|arg| arg.spread.is_some())
    {
        return None;
    }
    let method = match &member.prop {
        MemberProp::Ident(name) => name.sym.as_ref(),
        _ => return None,
    };
    let arguments_match = match method {
        "apply" => {
            call.args.len() == 2
                && runtime_ident(&call.args[0].expr, "exports", unresolved_mark)
                && matches!(strip_parens(&call.args[1].expr), Expr::Array(array) if array.elems.is_empty())
        }
        "call" => {
            call.args.len() == 4
                && call
                    .args
                    .iter()
                    .zip(["exports", "require", "exports", "module"])
                    .all(|(arg, name)| runtime_ident(&arg.expr, name, unresolved_mark))
        }
        _ => false,
    };
    if !arguments_match {
        return None;
    }
    let [Stmt::Return(returned)] = factory.function.body.as_ref()?.stmts.as_slice() else {
        return None;
    };
    let Expr::Ident(value) = strip_parens(returned.arg.as_ref()?) else {
        return None;
    };
    // Keep the read at exactly the invocation site. No non-undefined or
    // stable-function inference is needed: the original result guard stays.
    (value.sym != "arguments" && value.ctxt.outer() != unresolved_mark).then(|| value.clone())
}
