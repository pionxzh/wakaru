//! Named export surface of a module that stayed CommonJS.
//!
//! `collect_module_facts` only reads ESM `import` / `export` declarations.
//! A provider `UnEsm` leaves as CommonJS therefore has an empty export list,
//! and `run_provider_namespace_repair` will not turn a synthesized default
//! import into `import * as`. This collector records one narrower fact: the
//! post-recovery AST itself has an `__esModule` marker, an enumerable named
//! surface, and no default. Node's default import of an unmarked CommonJS
//! module is `module.exports`, so the marker is required.

use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinExpr, BinaryOp, CallExpr, Callee, Class, CondExpr, Expr, FnExpr,
    ForInStmt, ForOfStmt, Function, Ident, Lit, MemberExpr, MemberProp, Module, ModuleItem,
    OptChainBase, SimpleAssignTarget, UnaryOp, UpdateExpr,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::rules::contains_local_self_require;
use crate::rules::eval_utils::direct_eval_call_source;
use crate::rules::expr_utils::is_unresolved_ident;
use crate::rules::is_cjs_this_helper_guard;
use crate::rules::un_enum::{
    direct_cc_rf_marker, enclosing_cc_rf_push_span, first_arg_is_unresolved_module,
    is_cc_rf_method_callee, CcRfMarker,
};
use crate::rules::un_esmodule_flag::has_top_level_esmodule_flag;
use crate::utils::paren::strip_parens;

/// Whether `module` is a fail-closed CommonJS file whose consumers may read
/// its named exports through a namespace import.
pub(crate) fn collect_commonjs_marked_named_without_default(
    module: &Module,
    unresolved_mark: Mark,
    current_filename: Option<&str>,
) -> bool {
    // Any ESM declaration is the existing fact path. This flag must not sit
    // beside `export *`, a re-export, or a recovered named export.
    if module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
    {
        return false;
    }
    if !has_top_level_esmodule_flag(module, unresolved_mark) {
        return false;
    }
    if direct_eval_call_source_in(module) {
        return false;
    }
    // A self-require aliases the exports object before this module finishes
    // initializing. That alias can install `default` without a static write.
    if contains_local_self_require(module, unresolved_mark, current_filename) {
        return false;
    }

    let qualifying_pushes: Vec<_> = module
        .body
        .iter()
        .filter_map(|item| match direct_cc_rf_marker(item, unresolved_mark) {
            Some(CcRfMarker::Push {
                skippable_span: Some(span),
            }) => Some(span),
            _ => None,
        })
        .collect();
    // Only the one direct top-level push may mention `module`. A push nested
    // in a call or initializer has a different span and stays an escape.
    let allowed_push_span = if qualifying_pushes.len() == 1 {
        Some(qualifying_pushes[0])
    } else {
        None
    };

    let mut scan = SurfaceScan {
        unresolved_mark,
        item_index: 0,
        function_depth: 0,
        this_barrier: 0,
        conditional: 0,
        allow_surface: false,
        allow_module: false,
        allowed_push_span,
        reject: false,
        surface_items: HashSet::new(),
        enumerable_items: HashSet::new(),
    };
    for (index, item) in module.body.iter().enumerate() {
        scan.item_index = index;
        item.visit_with(&mut scan);
        if scan.reject {
            return false;
        }
    }
    if scan.enumerable_items.is_empty() {
        return false;
    }

    let pop_count = module
        .body
        .iter()
        .filter(|item| {
            matches!(
                direct_cc_rf_marker(item, unresolved_mark),
                Some(CcRfMarker::Pop)
            )
        })
        .count();
    if qualifying_pushes.is_empty() && pop_count == 0 {
        return true;
    }
    // `cc._RF.pop()` may replace an empty `module.exports`. One push whose
    // first argument is the unresolved `module`, one matching pop, and every
    // load-time surface statement inside that frame.
    let [push_span] = qualifying_pushes.as_slice() else {
        return false;
    };
    if pop_count != 1 {
        return false;
    }
    scan.surface_items.iter().all(|index| {
        enclosing_cc_rf_push_span(
            module.body[..*index].iter(),
            module.body[*index + 1..].iter(),
            unresolved_mark,
        ) == Some(*push_span)
    }) && scan.enumerable_items.iter().all(|index| {
        enclosing_cc_rf_push_span(
            module.body[..*index].iter(),
            module.body[*index + 1..].iter(),
            unresolved_mark,
        ) == Some(*push_span)
    })
}

fn direct_eval_call_source_in(module: &Module) -> bool {
    struct Finder {
        found: bool,
    }
    impl Visit for Finder {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if direct_eval_call_source(call).is_some() {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }
    let mut finder = Finder { found: false };
    module.visit_with(&mut finder);
    finder.found
}

struct SurfaceScan {
    unresolved_mark: Mark,
    item_index: usize,
    function_depth: usize,
    /// Ordinary functions and class bodies bind their own `this`.
    this_barrier: usize,
    conditional: usize,
    allow_surface: bool,
    allow_module: bool,
    allowed_push_span: Option<swc_core::common::Span>,
    reject: bool,
    surface_items: HashSet<usize>,
    enumerable_items: HashSet<usize>,
}

impl SurfaceScan {
    fn at_load_time(&self) -> bool {
        self.function_depth == 0
    }

    fn note_named(&mut self, name: &Atom, enumerable: bool) {
        if name == "default" {
            self.reject = true;
            return;
        }
        if name == "__esModule" || !self.at_load_time() {
            return;
        }
        self.surface_items.insert(self.item_index);
        if enumerable && self.conditional == 0 {
            self.enumerable_items.insert(self.item_index);
        }
    }

    fn with_surface_object(&mut self, visit: impl FnOnce(&mut Self)) {
        let previous = self.allow_surface;
        self.allow_surface = true;
        visit(self);
        self.allow_surface = previous;
    }

    fn with_module_ident(&mut self, visit: impl FnOnce(&mut Self)) {
        let previous = self.allow_module;
        self.allow_module = true;
        visit(self);
        self.allow_module = previous;
    }

    fn enter_nested(&mut self, own_this: bool, visit: impl FnOnce(&mut Self)) {
        self.function_depth += 1;
        if own_this {
            self.this_barrier += 1;
        }
        visit(self);
        if own_this {
            self.this_barrier -= 1;
        }
        self.function_depth -= 1;
    }

    fn object_method(&self, call: &CallExpr, method: &str) -> bool {
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };
        let Expr::Member(member) = strip_parens(callee) else {
            return false;
        };
        matches!(strip_parens(&member.obj), Expr::Ident(object)
            if is_unresolved_ident(object, "Object", self.unresolved_mark))
            && matches!(&member.prop, MemberProp::Ident(name) if name.sym == *method)
    }

    fn export_star_callee(&self, call: &CallExpr) -> bool {
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };
        matches!(strip_parens(callee), Expr::Ident(ident)
            if is_unresolved_ident(ident, "__exportStar", self.unresolved_mark))
    }
}

fn is_exports_ident(expr: &Expr, unresolved_mark: Mark) -> bool {
    matches!(strip_parens(expr), Expr::Ident(ident)
        if is_unresolved_ident(ident, "exports", unresolved_mark))
}

fn is_module_exports(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    matches!(strip_parens(&member.obj), Expr::Ident(object)
        if is_unresolved_ident(object, "module", unresolved_mark))
        && is_static_name(&member.prop, "exports")
}

fn is_export_object(expr: &Expr, unresolved_mark: Mark) -> bool {
    is_exports_ident(expr, unresolved_mark) || is_module_exports(expr, unresolved_mark)
}

fn is_static_name(prop: &MemberProp, name: &str) -> bool {
    match prop {
        MemberProp::Ident(ident) => ident.sym == *name,
        MemberProp::Computed(computed) => {
            matches!(strip_parens(&computed.expr), Expr::Lit(Lit::Str(value))
            if value.value == *name)
        }
        MemberProp::PrivateName(_) => false,
    }
}

fn static_prop_name(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(ident) => Some(ident.sym.clone()),
        MemberProp::Computed(computed) => match strip_parens(&computed.expr) {
            Expr::Lit(Lit::Str(value)) => Some(value.value.to_string_lossy().into()),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

fn prop_is_dynamic(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Computed(computed) => {
            !matches!(strip_parens(&computed.expr), Expr::Lit(Lit::Str(_)))
        }
        _ => false,
    }
}

fn loose_true(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Lit(Lit::Bool(value)) => value.value,
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => {
            matches!(strip_parens(&unary.arg), Expr::Lit(Lit::Num(value)) if value.value == 0.0)
        }
        _ => false,
    }
}

fn descriptor_is_enumerable(expr: &Expr) -> bool {
    let Expr::Object(object) = strip_parens(expr) else {
        return false;
    };
    object.props.iter().any(|prop| {
        let swc_core::ecma::ast::PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        let swc_core::ecma::ast::Prop::KeyValue(entry) = prop.as_ref() else {
            return false;
        };
        matches!(&entry.key, swc_core::ecma::ast::PropName::Ident(name) if name.sym == "enumerable")
            && loose_true(&entry.value)
    })
}

impl Visit for SurfaceScan {
    fn visit_function(&mut self, function: &Function) {
        self.enter_nested(true, |scan| function.visit_children_with(scan));
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        self.enter_nested(false, |scan| arrow.visit_children_with(scan));
    }

    fn visit_fn_expr(&mut self, function: &FnExpr) {
        // `visit_function` already covers the body. Visiting the wrapper
        // again would double-count. The ident is not an export surface.
        function.function.visit_with(self);
    }

    fn visit_class(&mut self, class: &Class) {
        for decorator in &class.decorators {
            decorator.visit_with(self);
        }
        if let Some(super_class) = &class.super_class {
            super_class.visit_with(self);
        }
        self.enter_nested(true, |scan| class.body.visit_with(scan));
    }

    fn visit_with_stmt(&mut self, _: &swc_core::ecma::ast::WithStmt) {
        self.reject = true;
    }

    fn visit_if_stmt(&mut self, stmt: &swc_core::ecma::ast::IfStmt) {
        stmt.test.visit_with(self);
        self.conditional += 1;
        stmt.cons.visit_with(self);
        if let Some(alt) = &stmt.alt {
            alt.visit_with(self);
        }
        self.conditional -= 1;
    }

    fn visit_for_stmt(&mut self, stmt: &swc_core::ecma::ast::ForStmt) {
        self.conditional += 1;
        stmt.visit_children_with(self);
        self.conditional -= 1;
    }

    fn visit_while_stmt(&mut self, stmt: &swc_core::ecma::ast::WhileStmt) {
        self.conditional += 1;
        stmt.visit_children_with(self);
        self.conditional -= 1;
    }

    fn visit_do_while_stmt(&mut self, stmt: &swc_core::ecma::ast::DoWhileStmt) {
        self.conditional += 1;
        stmt.visit_children_with(self);
        self.conditional -= 1;
    }

    fn visit_switch_stmt(&mut self, stmt: &swc_core::ecma::ast::SwitchStmt) {
        self.conditional += 1;
        stmt.visit_children_with(self);
        self.conditional -= 1;
    }

    fn visit_try_stmt(&mut self, stmt: &swc_core::ecma::ast::TryStmt) {
        self.conditional += 1;
        stmt.visit_children_with(self);
        self.conditional -= 1;
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        self.conditional += 1;
        if is_export_object(&stmt.right, self.unresolved_mark) {
            self.with_surface_object(|scan| stmt.right.visit_with(scan));
        } else {
            stmt.right.visit_with(self);
        }
        stmt.left.visit_with(self);
        stmt.body.visit_with(self);
        self.conditional -= 1;
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        self.conditional += 1;
        if is_export_object(&stmt.right, self.unresolved_mark) {
            self.with_surface_object(|scan| stmt.right.visit_with(scan));
        } else {
            stmt.right.visit_with(self);
        }
        stmt.left.visit_with(self);
        stmt.body.visit_with(self);
        self.conditional -= 1;
    }

    fn visit_cond_expr(&mut self, expr: &CondExpr) {
        expr.test.visit_with(self);
        self.conditional += 1;
        expr.cons.visit_with(self);
        expr.alt.visit_with(self);
        self.conditional -= 1;
    }

    fn visit_bin_expr(&mut self, expr: &BinExpr) {
        if self.this_barrier == 0 && is_cjs_this_helper_guard(expr) && self.at_load_time() {
            self.surface_items.insert(self.item_index);
            return;
        }
        let logical = matches!(
            expr.op,
            BinaryOp::LogicalAnd | BinaryOp::LogicalOr | BinaryOp::NullishCoalescing
        );
        if logical {
            self.conditional += 1;
        }
        expr.visit_children_with(self);
        if logical {
            self.conditional -= 1;
        }
    }

    fn visit_unary_expr(&mut self, expr: &swc_core::ecma::ast::UnaryExpr) {
        if expr.op == UnaryOp::TypeOf && matches!(strip_parens(&expr.arg), Expr::This(_)) {
            if self.this_barrier == 0 && self.at_load_time() {
                self.surface_items.insert(self.item_index);
            }
            return;
        }
        if expr.op == UnaryOp::Delete && surface_delete(&expr.arg, self.unresolved_mark) {
            self.reject = true;
        }
        expr.visit_children_with(self);
    }

    fn visit_this_expr(&mut self, _: &swc_core::ecma::ast::ThisExpr) {
        if self.this_barrier == 0 {
            self.reject = true;
        }
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if self.this_barrier == 0 && is_unresolved_ident(ident, "arguments", self.unresolved_mark) {
            self.reject = true;
        }
        if is_unresolved_ident(ident, "exports", self.unresolved_mark) && !self.allow_surface {
            self.reject = true;
        }
        if is_unresolved_ident(ident, "module", self.unresolved_mark) && !self.allow_module {
            self.reject = true;
        }
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if is_module_exports_object(member, self.unresolved_mark) {
            // Bare `exports` rejects unless it is the object of a static member.
            // `module.exports` is the same object and needs the same proof.
            if !self.allow_surface {
                self.reject = true;
            }
            self.with_module_ident(|scan| member.obj.visit_with(scan));
            return;
        }
        if is_export_object(&member.obj, self.unresolved_mark) {
            if prop_is_dynamic(&member.prop) {
                self.reject = true;
            }
            self.with_surface_object(|scan| member.obj.visit_with(scan));
            if let MemberProp::Computed(computed) = &member.prop {
                computed.expr.visit_with(self);
            }
            return;
        }
        if matches!(strip_parens(&member.obj), Expr::This(_))
            && static_prop_name(&member.prop).as_deref() == Some("default")
        {
            // A write is caught again on the assignment. A read of
            // `this.default` is still a default-shaped access we do not prove.
            self.reject = true;
            return;
        }
        member.visit_children_with(self);
    }

    fn visit_opt_chain_expr(&mut self, chain: &swc_core::ecma::ast::OptChainExpr) {
        match &*chain.base {
            OptChainBase::Member(member) => member.visit_with(self),
            OptChainBase::Call(call) => call.visit_with(self),
        }
    }

    fn visit_update_expr(&mut self, expr: &UpdateExpr) {
        if update_touches_surface(&expr.arg, self.unresolved_mark) {
            self.reject = true;
        }
        expr.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, expr: &swc_core::ecma::ast::AssignExpr) {
        match classify_target(&expr.left, self.unresolved_mark) {
            TargetClass::ReplaceObject => self.reject = true,
            TargetClass::Dynamic => self.reject = true,
            TargetClass::ThisDefault => self.reject = true,
            TargetClass::Prop(name) => {
                if expr.op != AssignOp::Assign {
                    self.reject = true;
                } else {
                    self.note_named(&name, true);
                }
            }
            TargetClass::Other => {}
        }
        expr.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if direct_eval_call_source(call).is_some() {
            self.reject = true;
            return;
        }
        let exempt_push = self.allowed_push_span == Some(call.span)
            && call_callee_is(call, |callee| {
                is_cc_rf_method_callee(callee, "push", self.unresolved_mark)
            })
            && first_arg_is_unresolved_module(call, self.unresolved_mark);
        if exempt_push {
            self.with_module_ident(|scan| call.args[0].visit_with(scan));
            call.callee.visit_with(self);
            for arg in call.args.iter().skip(1) {
                arg.visit_with(self);
            }
            return;
        }
        if self.object_method(call, "defineProperty")
            && call.args.len() == 3
            && no_spread(call)
            && is_export_object(&call.args[0].expr, self.unresolved_mark)
        {
            match static_prop_name_from_expr(&call.args[1].expr) {
                Some(name) if name == "__esModule" => {}
                Some(name) => self.note_named(&name, descriptor_is_enumerable(&call.args[2].expr)),
                None => self.reject = true,
            }
            self.with_surface_object(|scan| call.args[0].visit_with(scan));
            call.args[1].visit_with(self);
            call.args[2].visit_with(self);
            call.callee.visit_with(self);
            return;
        }
        if self.object_method(call, "defineProperties")
            && !call.args.is_empty()
            && no_spread(call)
            && is_export_object(&call.args[0].expr, self.unresolved_mark)
        {
            // The `__esModule` marker form is not a named surface. Any
            // other bulk define shares the object and is unproven.
            if !is_only_esmodule_define_properties(call) {
                self.reject = true;
            }
            self.with_surface_object(|scan| call.args[0].visit_with(scan));
            for arg in call.args.iter().skip(1) {
                arg.visit_with(self);
            }
            call.callee.visit_with(self);
            return;
        }
        if (self.object_method(call, "assign") || self.export_star_callee(call))
            && call.args.iter().any(|arg| {
                arg.spread.is_none() && is_export_object(&arg.expr, self.unresolved_mark)
            })
        {
            self.reject = true;
        }
        call.visit_children_with(self);
    }
}

fn call_callee_is(call: &CallExpr, pred: impl FnOnce(&Expr) -> bool) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    pred(callee)
}

fn no_spread(call: &CallExpr) -> bool {
    call.args.iter().all(|arg| arg.spread.is_none())
}

fn is_module_exports_object(member: &MemberExpr, unresolved_mark: Mark) -> bool {
    is_static_name(&member.prop, "exports")
        && matches!(strip_parens(&member.obj), Expr::Ident(object)
            if is_unresolved_ident(object, "module", unresolved_mark))
}

fn static_prop_name_from_expr(expr: &Expr) -> Option<Atom> {
    match strip_parens(expr) {
        Expr::Lit(Lit::Str(value)) => Some(value.value.to_string_lossy().into()),
        _ => None,
    }
}

fn is_only_esmodule_define_properties(call: &CallExpr) -> bool {
    if call.args.len() != 2 || !no_spread(call) {
        return false;
    }
    let Expr::Object(object) = strip_parens(&call.args[1].expr) else {
        return false;
    };
    let mut saw_marker = false;
    for prop in &object.props {
        let swc_core::ecma::ast::PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        let swc_core::ecma::ast::Prop::KeyValue(entry) = prop.as_ref() else {
            return false;
        };
        let name = match &entry.key {
            swc_core::ecma::ast::PropName::Ident(ident) => ident.sym.clone(),
            swc_core::ecma::ast::PropName::Str(value) => value.value.to_string_lossy().into(),
            _ => return false,
        };
        if name != "__esModule" {
            return false;
        }
        saw_marker = true;
    }
    saw_marker
}

enum TargetClass {
    ReplaceObject,
    Dynamic,
    ThisDefault,
    Prop(Atom),
    Other,
}

fn classify_target(target: &AssignTarget, unresolved_mark: Mark) -> TargetClass {
    let AssignTarget::Simple(simple) = target else {
        return TargetClass::Other;
    };
    match simple {
        SimpleAssignTarget::Ident(ident)
            if is_unresolved_ident(&ident.id, "exports", unresolved_mark) =>
        {
            TargetClass::ReplaceObject
        }
        SimpleAssignTarget::Member(member) => classify_member(member, unresolved_mark),
        _ => TargetClass::Other,
    }
}

fn classify_member(member: &MemberExpr, unresolved_mark: Mark) -> TargetClass {
    if is_module_exports_object(member, unresolved_mark) {
        return TargetClass::ReplaceObject;
    }
    if matches!(strip_parens(&member.obj), Expr::This(_)) {
        // `this.name =` is not a proven `exports.name` write, and `this.default`
        // is a default write wherever it appears.
        return TargetClass::ThisDefault;
    }
    if is_export_object(&member.obj, unresolved_mark)
        || is_module_exports(&member.obj, unresolved_mark)
    {
        if prop_is_dynamic(&member.prop) {
            return TargetClass::Dynamic;
        }
        if let Some(name) = static_prop_name(&member.prop) {
            return TargetClass::Prop(name);
        }
    }
    // `module.exports.name`: obj is `module.exports`, which `is_export_object` covers.
    TargetClass::Other
}

fn surface_delete(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    is_export_object(&member.obj, unresolved_mark)
        || is_module_exports(&member.obj, unresolved_mark)
}

fn update_touches_surface(expr: &Expr, unresolved_mark: Mark) -> bool {
    match strip_parens(expr) {
        Expr::Ident(ident) if is_unresolved_ident(ident, "exports", unresolved_mark) => true,
        Expr::Member(member) => {
            is_export_object(&member.obj, unresolved_mark)
                || is_module_exports(&member.obj, unresolved_mark)
                || is_module_exports_object(member, unresolved_mark)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, FileName, Globals, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    use super::*;

    fn marked(source: &str) -> bool {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(
                FileName::Custom("provider.js".into()).into(),
                source.to_string(),
            );
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*file),
                None,
            );
            let mut module = Parser::new_from(lexer)
                .parse_module()
                .expect("fixture should parse");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            collect_commonjs_marked_named_without_default(
                &module,
                unresolved_mark,
                Some("provider.js"),
            )
        })
    }

    const MARKER: &str = r#"Object.defineProperty(exports, "__esModule", { value: true });"#;

    #[test]
    fn enumerable_named_write_with_marker_and_guard_is_proven() {
        let source = format!(
            r#"
"use strict";
cc._RF.push(module, "id", "Provider");
{MARKER}
exports.name = void 0;
var guard = (this && this.__decorate) || function () {{}};
observe(typeof this);
exports.name = function (t, e) {{ return t + e; }};
cc._RF.pop();
"#
        );
        assert!(marked(&source), "{source}");
    }

    #[test]
    fn named_write_without_a_module_ident_needs_no_marker_frame() {
        let source = format!(
            r#"
"use strict";
{MARKER}
exports.name = function (t, e) {{ return t + e; }};
observe(typeof this);
"#
        );
        assert!(marked(&source), "{source}");
    }

    #[test]
    fn already_esm_and_unmarked_commonjs_are_not_this_fact() {
        assert!(!marked("export const name = 1;\n"));
        assert!(!marked("exports.name = 1;\nobserve(typeof this);\n"));
    }

    #[test]
    fn default_writes_reject_the_surface() {
        for extra in [
            "exports.default = function E() {};",
            "function later() { exports.default = 1; }",
            "function later() { module.exports.default = 1; }",
            "function later() { exports[\"default\"] = 1; }",
            "function later() { Object.defineProperty(exports, \"default\", { value: 1 }); }",
            "function later() { this.default = 1; }",
            "module.exports = function D() {};",
        ] {
            let source = format!(
                r#"
"use strict";
{MARKER}
exports.name = 1;
{extra}
observe(typeof this);
"#
            );
            assert!(!marked(&source), "{extra}");
        }
    }

    #[test]
    fn unproven_shapes_reject_the_surface() {
        for extra in [
            "exports[key] = 1;",
            "delete exports.foo;",
            "Object.assign(exports, extra);",
            "__exportStar(require(\"./dep.js\"), exports);",
            "var alias = exports; alias.foo = 1;",
            "observe(exports);",
            "unknown(module);",
            "eval(\"exports.name = 2\");",
            "with (obj) { exports.extra = 1; }",
            "use(arguments);",
            "(function (r) { r.x = 1; })(this);",
        ] {
            let source = format!(
                r#"
"use strict";
cc._RF.push(module, "id", "Provider");
{MARKER}
exports.name = 1;
{extra}
observe(typeof this);
cc._RF.pop();
"#
            );
            assert!(!marked(&source), "{extra}\n{source}");
        }
    }

    #[test]
    fn only_a_non_enumerable_getter_is_not_a_named_surface() {
        // `pop()` replaces `module.exports` when the object has no enumerable keys.
        let source = format!(
            r#"
"use strict";
cc._RF.push(module, "id", "Provider");
{MARKER}
Object.defineProperty(exports, "getter", {{ get: function () {{ return 1; }} }});
observe(typeof this);
cc._RF.pop();
"#
        );
        assert!(!marked(&source), "{source}");
    }

    #[test]
    fn marker_frame_must_enclose_the_named_write() {
        for source in [
            format!(
                r#"
"use strict";
function nested() {{ cc._RF.push(module, "id", "Provider"); cc._RF.pop(); }}
{MARKER}
exports.name = 1;
observe(typeof this);
"#
            ),
            format!(
                r#"
"use strict";
{MARKER}
exports.name = 1;
observe(typeof this);
cc._RF.push(module, "id", "Provider");
cc._RF.pop();
"#
            ),
            format!(
                r#"
"use strict";
cc._RF.push(module, "id", "Provider");
cc._RF.pop();
{MARKER}
exports.name = 1;
observe(typeof this);
"#
            ),
            format!(
                r#"
"use strict";
cc._RF.push(module, "id", "Provider");
{MARKER}
exports.name = 1;
observe(typeof this);
"#
            ),
            format!(
                r#"
"use strict";
cc._RF.push(module, "id", "Provider");
cc._RF.push(module, "id2", "Other");
{MARKER}
exports.name = 1;
observe(typeof this);
cc._RF.pop();
cc._RF.pop();
"#
            ),
            format!(
                r#"
"use strict";
cc._RF.push(module, "id", "Provider");
{MARKER}
observe(typeof this);
cc._RF.pop();
exports.name = 1;
"#
            ),
        ] {
            assert!(!marked(&source), "{source}");
        }
    }

    #[test]
    fn export_object_aliases_and_nested_pushes_are_not_proven() {
        for source in [
            format!(
                r#"
"use strict";
{MARKER}
exports.name = 1;
observe(module.exports);
observe(typeof this);
"#
            ),
            format!(
                r#"
"use strict";
{MARKER}
exports.name = 1;
var alias = module.exports;
alias.default = 1;
observe(typeof this);
"#
            ),
            format!(
                r#"
"use strict";
{MARKER}
exports.name = 1;
var later = () => {{ arguments[0].default = 1; }};
observe(typeof this);
"#
            ),
            format!(
                r#"
"use strict";
{MARKER}
exports.name = 1;
var self = require("./provider.js");
observe(typeof this);
"#
            ),
            format!(
                r#"
"use strict";
{MARKER}
exports.name = 1;
register(cc._RF.push(module, "id", "Provider"));
observe(typeof this);
"#
            ),
            format!(
                r#"
"use strict";
{MARKER}
exports.name = 1;
var frame = cc._RF.push(module, "id", "Provider");
observe(typeof this);
"#
            ),
            format!(
                r#"
"use strict";
cc._RF.push(module, "id", "Provider");
{MARKER}
exports.name = 1;
observe(typeof this);
cc._RF.pop();
register(cc._RF.push(module, "id", "Provider"));
"#
            ),
        ] {
            assert!(!marked(&source), "{source}");
        }
    }
}
