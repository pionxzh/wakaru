use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    BindingIdent, CallExpr, Callee, Class, ClassMember, ClassProp, Expr, ExprOrSpread, ExprStmt,
    IdentName, KeyValueProp, Lit, MemberExpr, MemberProp, Module, Pat, Prop, PropName,
    PropOrSpread, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith};

use super::babel_helper_utils::{
    collect_helper_dependencies, collect_helpers_of_kind, helpers_with_remaining_refs,
    remove_helper_declarations, BabelHelperKind, BindingKey,
};
use super::helper_matcher::binding_key;
use super::RewriteLevel;

/// Inline `__init*()` method bodies into the constructor.
///
/// Babel/SWC class field transpilation produces:
/// ```js
/// class Foo {
///     __init() { this._x = 1; }
///     __init2() { this._y = 2; }
///     constructor() {
///         Foo.prototype.__init.call(this);
///         Foo.prototype.__init2.call(this);
///     }
/// }
/// ```
/// This rule inlines them back:
/// ```js
/// class Foo {
///     constructor() {
///         this._x = 1;
///         this._y = 2;
///     }
/// }
/// ```
pub struct UnClassFields {
    level: RewriteLevel,
    unresolved_mark: Mark,
    define_property_helpers: HashSet<BindingKey>,
}

impl UnClassFields {
    pub fn new(level: RewriteLevel) -> Self {
        Self::new_with_mark(Mark::new(), level)
    }

    pub fn new_with_mark(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            level,
            unresolved_mark,
            define_property_helpers: HashSet::new(),
        }
    }
}

impl Default for UnClassFields {
    fn default() -> Self {
        Self::new(RewriteLevel::Standard)
    }
}

impl VisitMut for UnClassFields {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let helpers = collect_helpers_of_kind(module, BabelHelperKind::DefineProperty);
        let previous_helpers = std::mem::replace(
            &mut self.define_property_helpers,
            helpers.keys().cloned().collect(),
        );
        module.visit_mut_children_with(self);
        if !helpers.is_empty() {
            let remaining = helpers_with_remaining_refs(module, &helpers);
            let removable_roots: HashMap<BindingKey, BabelHelperKind> = helpers
                .into_iter()
                .filter(|(key, _)| !remaining.contains(key))
                .collect();
            let removable_dependencies = collect_helper_dependencies(module, &removable_roots);
            let removable_helpers: HashMap<BindingKey, BabelHelperKind> = removable_roots
                .into_iter()
                .chain(removable_dependencies)
                .collect();
            let remaining = helpers_with_remaining_refs(module, &removable_helpers);
            let safe_to_remove: HashMap<BindingKey, BabelHelperKind> = removable_helpers
                .into_iter()
                .filter(|(key, _)| !remaining.contains(key))
                .collect();
            if !safe_to_remove.is_empty() {
                remove_helper_declarations(&mut module.body, &safe_to_remove);
            }
        }
        self.define_property_helpers = previous_helpers;
    }

    fn visit_mut_class(&mut self, class: &mut Class) {
        class.visit_mut_children_with(self);

        // Collect __init* method bodies
        let mut init_bodies: HashMap<Atom, Vec<Stmt>> = HashMap::new();
        for member in &class.body {
            let ClassMember::Method(method) = member else {
                continue;
            };
            let Some(name) = prop_name_str(&method.key) else {
                continue;
            };
            if !name.starts_with("__init") {
                continue;
            }
            if method.is_static {
                continue;
            }
            let Some(body) = &method.function.body else {
                continue;
            };
            // All statements must be `this.X = expr` assignments
            if body.stmts.is_empty() {
                continue;
            }
            let all_this_assigns = body.stmts.iter().all(is_this_assignment);
            if !all_this_assigns {
                continue;
            }
            init_bodies.insert(Atom::from(name), body.stmts.clone());
        }

        // Find constructor and inline the __init calls
        let class_name = self.find_class_name(class);
        let mut inlined_names: std::collections::HashSet<Atom> = std::collections::HashSet::new();

        if !init_bodies.is_empty() {
            for member in &mut class.body {
                let ClassMember::Constructor(ctor) = member else {
                    continue;
                };
                let Some(body) = &mut ctor.body else {
                    continue;
                };

                let mut new_stmts = Vec::with_capacity(body.stmts.len());
                for stmt in body.stmts.drain(..) {
                    if let Some(init_name) = extract_prototype_init_call(&stmt, &class_name) {
                        if let Some(init_stmts) = init_bodies.get(&init_name) {
                            new_stmts.extend(init_stmts.iter().cloned());
                            inlined_names.insert(init_name);
                            continue;
                        }
                    }
                    new_stmts.push(stmt);
                }
                body.stmts = new_stmts;
            }
        }

        if !inlined_names.is_empty() {
            // Remove only the __init* methods that were actually inlined
            class.body.retain(|member| {
                let ClassMember::Method(method) = member else {
                    return true;
                };
                if method.is_static {
                    return true;
                }
                let Some(name) = prop_name_str(&method.key) else {
                    return true;
                };
                if inlined_names.contains(&Atom::from(name.as_str())) {
                    return false; // remove
                }
                true
            });
        }

        if inlined_names.is_empty()
            && self.level >= RewriteLevel::Standard
            && class.super_class.is_none()
        {
            promote_constructor_field_assignments(
                class,
                &self.define_property_helpers,
                self.unresolved_mark,
            );
        }
    }
}

impl UnClassFields {
    fn find_class_name(&self, _class: &Class) -> Option<Atom> {
        // Classes in class declarations have their name set by the parent node,
        // not on the Class itself. We'll match by checking the prototype call pattern.
        None // Will match any class name in extract_prototype_init_call
    }
}

fn prop_name_str(key: &swc_core::ecma::ast::PropName) -> Option<String> {
    match key {
        swc_core::ecma::ast::PropName::Ident(id) => Some(id.sym.to_string()),
        swc_core::ecma::ast::PropName::Str(s) => s.value.as_str().map(|s| s.to_string()),
        _ => None,
    }
}

fn promote_constructor_field_assignments(
    class: &mut Class,
    define_property_helpers: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) {
    let Some(ctor_index) = class
        .body
        .iter()
        .position(|member| matches!(member, ClassMember::Constructor(_)))
    else {
        return;
    };

    let (class_props, remove_empty_ctor) = {
        let ClassMember::Constructor(ctor) = &mut class.body[ctor_index] else {
            return;
        };
        let Some(body) = &mut ctor.body else {
            return;
        };
        let blocked_bindings = constructor_blocked_bindings(&ctor.params);
        let mut class_props = Vec::new();
        let mut consumed = 0;

        for stmt in &body.stmts {
            let Some((key, value)) =
                extract_instance_field_initializer(stmt, define_property_helpers, unresolved_mark)
            else {
                break;
            };
            if expr_uses_blocked_binding(&value, &blocked_bindings) {
                break;
            }
            class_props.push(ClassMember::ClassProp(ClassProp {
                span: DUMMY_SP,
                key,
                value: Some(value),
                type_ann: None,
                is_static: false,
                decorators: Vec::new(),
                accessibility: None,
                is_abstract: false,
                is_optional: false,
                is_override: false,
                readonly: false,
                declare: false,
                definite: false,
            }));
            consumed += 1;
        }

        if class_props.is_empty() {
            return;
        }

        body.stmts.drain(0..consumed);
        let remove_empty_ctor = body.stmts.is_empty() && ctor.params.is_empty();
        (class_props, remove_empty_ctor)
    };

    if remove_empty_ctor {
        class.body.remove(ctor_index);
    }
    for (offset, prop) in class_props.into_iter().enumerate() {
        class.body.insert(ctor_index + offset, prop);
    }
}

fn extract_instance_field_initializer(
    stmt: &Stmt,
    define_property_helpers: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> Option<(PropName, Box<Expr>)> {
    extract_babel_instance_field_initializer(stmt, define_property_helpers).or_else(|| {
        extract_object_define_property_instance_field_initializer(stmt, unresolved_mark)
    })
}

fn constructor_blocked_bindings(
    params: &[swc_core::ecma::ast::ParamOrTsParamProp],
) -> Vec<(Atom, SyntaxContext)> {
    let mut bindings = Vec::new();
    for param in params {
        if let swc_core::ecma::ast::ParamOrTsParamProp::Param(param) = param {
            collect_pat_binding_keys(&param.pat, &mut bindings);
        }
    }
    bindings
}

fn collect_pat_binding_keys(pat: &Pat, bindings: &mut Vec<(Atom, SyntaxContext)>) {
    match pat {
        Pat::Ident(BindingIdent { id, .. }) => bindings.push((id.sym.clone(), id.ctxt)),
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_binding_keys(elem, bindings);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                        collect_pat_binding_keys(&kv.value, bindings);
                    }
                    swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                        bindings.push((assign.key.sym.clone(), assign.key.ctxt));
                    }
                    swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                        collect_pat_binding_keys(&rest.arg, bindings);
                    }
                }
            }
        }
        Pat::Rest(rest) => collect_pat_binding_keys(&rest.arg, bindings),
        Pat::Assign(assign) => collect_pat_binding_keys(&assign.left, bindings),
        _ => {}
    }
}

fn extract_babel_instance_field_initializer(
    stmt: &Stmt,
    define_property_helpers: &HashSet<BindingKey>,
) -> Option<(PropName, Box<Expr>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = &**expr else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(callee_ident) = callee.as_ref() else {
        return None;
    };
    if !define_property_helpers.contains(&binding_key(callee_ident)) {
        return None;
    }
    if call.args.len() != 3 || call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }
    let [ExprOrSpread { expr: obj, .. }, ExprOrSpread { expr: key, .. }, ExprOrSpread { expr: value, .. }] =
        call.args.as_slice()
    else {
        return None;
    };
    if !matches!(obj.as_ref(), Expr::This(_)) {
        return None;
    }
    let key = field_key_from_expr(key)?;
    Some((key, value.clone()))
}

fn extract_object_define_property_instance_field_initializer(
    stmt: &Stmt,
    unresolved_mark: Mark,
) -> Option<(PropName, Box<Expr>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = &**expr else {
        return None;
    };
    if call.args.len() != 3 || call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }
    if !is_object_define_property_callee(&call.callee, unresolved_mark) {
        return None;
    }
    let [ExprOrSpread { expr: obj, .. }, ExprOrSpread { expr: key, .. }, ExprOrSpread {
        expr: descriptor, ..
    }] = call.args.as_slice()
    else {
        return None;
    };
    if !matches!(obj.as_ref(), Expr::This(_)) {
        return None;
    }
    let key = field_key_from_expr(key)?;
    let value = extract_class_field_descriptor_value(descriptor)?;
    Some((key, value))
}

fn is_object_define_property_callee(callee: &Callee, unresolved_mark: Mark) -> bool {
    let Callee::Expr(callee) = callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    obj.sym.as_ref() == "Object"
        && obj.ctxt.outer() == unresolved_mark
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "defineProperty")
}

fn extract_class_field_descriptor_value(descriptor: &Expr) -> Option<Box<Expr>> {
    let Expr::Object(object) = descriptor else {
        return None;
    };
    let mut value = None;
    let mut has_enumerable = false;
    let mut has_configurable = false;
    let mut has_writable = false;

    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(KeyValueProp {
            key,
            value: prop_value,
        }) = prop.as_ref()
        else {
            return None;
        };
        let name = prop_name_str(key)?;
        match name.as_str() {
            "value" => value = Some(prop_value.clone()),
            "enumerable" => {
                if !is_true_literal(prop_value) {
                    return None;
                }
                has_enumerable = true;
            }
            "configurable" => {
                if !is_true_literal(prop_value) {
                    return None;
                }
                has_configurable = true;
            }
            "writable" => {
                if !is_true_literal(prop_value) {
                    return None;
                }
                has_writable = true;
            }
            _ => return None,
        }
    }

    if has_enumerable && has_configurable && has_writable {
        value
    } else {
        None
    }
}

fn is_true_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Bool(bool_lit)) if bool_lit.value)
}

fn field_key_from_expr(expr: &Expr) -> Option<PropName> {
    match expr {
        Expr::Lit(swc_core::ecma::ast::Lit::Str(s)) => {
            let value = s.value.as_str()?;
            if is_identifier_name(value) {
                Some(PropName::Ident(IdentName::new(value.into(), DUMMY_SP)))
            } else {
                Some(PropName::Str(swc_core::ecma::ast::Str {
                    span: DUMMY_SP,
                    value: value.into(),
                    raw: None,
                }))
            }
        }
        _ => None,
    }
}

fn is_identifier_name(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn expr_uses_blocked_binding(expr: &Expr, blocked_bindings: &[(Atom, SyntaxContext)]) -> bool {
    struct BlockedBindingFinder<'a> {
        blocked_bindings: &'a [(Atom, SyntaxContext)],
        found: bool,
    }

    impl Visit for BlockedBindingFinder<'_> {
        fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
            if ident.sym.as_ref() == "arguments"
                || self
                    .blocked_bindings
                    .iter()
                    .any(|(sym, ctxt)| ident.sym == *sym && ident.ctxt == *ctxt)
            {
                self.found = true;
            }
        }
    }

    let mut finder = BlockedBindingFinder {
        blocked_bindings,
        found: false,
    };
    finder.visit_expr(expr);
    finder.found
}

/// Check if statement is `this.X = expr`
fn is_this_assignment(stmt: &Stmt) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = &**expr else {
        return false;
    };
    let swc_core::ecma::ast::AssignTarget::Simple(swc_core::ecma::ast::SimpleAssignTarget::Member(
        member,
    )) = &assign.left
    else {
        return false;
    };
    matches!(&*member.obj, Expr::This(_))
}

/// Extract `__initN` name from `ClassName.prototype.__initN.call(this)`
fn extract_prototype_init_call(stmt: &Stmt, _class_name: &Option<Atom>) -> Option<Atom> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee),
        args,
        ..
    }) = &**expr
    else {
        return None;
    };

    // callee: X.prototype.__initN.call
    let Expr::Member(MemberExpr {
        obj: call_obj,
        prop: MemberProp::Ident(call_prop),
        ..
    }) = &**callee
    else {
        return None;
    };
    if call_prop.sym.as_ref() != "call" {
        return None;
    }

    // call_obj: X.prototype.__initN
    let Expr::Member(MemberExpr {
        obj: proto_obj,
        prop: MemberProp::Ident(init_prop),
        ..
    }) = &**call_obj
    else {
        return None;
    };
    let init_name = &init_prop.sym;
    if !init_name.starts_with("__init") {
        return None;
    }

    // proto_obj: X.prototype
    let Expr::Member(MemberExpr {
        prop: MemberProp::Ident(proto_prop),
        ..
    }) = &**proto_obj
    else {
        return None;
    };
    if proto_prop.sym.as_ref() != "prototype" {
        return None;
    }

    // args must be exactly [this]
    if args.len() != 1 {
        return None;
    }
    if !matches!(&*args[0].expr, Expr::This(_)) {
        return None;
    }

    Some(init_name.clone())
}
