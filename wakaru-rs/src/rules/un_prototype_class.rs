use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BlockStmt, CallExpr, Callee, Class, ClassDecl, ClassMember,
    ClassMethod, Constructor, Decl, Expr, ExprOrSpread, ExprStmt, FnExpr, Function, IdentName,
    MemberProp, MethodKind, ModuleItem, Param, ParamOrTsParamProp, PropName, SimpleAssignTarget,
    Stmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub struct UnPrototypeClass;

impl VisitMut for UnPrototypeClass {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        transform_module_items(items);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        transform_stmts(stmts);
    }
}

// ============================================================
// Core transformation
// ============================================================

/// A constructor candidate with its associated prototype method assignments.
struct ClassCandidate {
    /// Index of the `function Foo() {}` declaration in the statement list.
    fn_decl_idx: usize,
    /// The constructor function name (e.g., "Foo").
    _name: Atom,
    /// Super class expression, if inheritance is detected.
    super_class: Option<Box<Expr>>,
    /// Super class name for `Parent.call(this, ...)` → `super(...)` rewriting.
    super_class_name: Option<Atom>,
    /// Indices of statements consumed by this class (prototype methods, inheritance, etc.).
    consumed_indices: HashSet<usize>,
    /// Collected class members.
    members: Vec<ClassMember>,
}

fn transform_module_items(items: &mut Vec<ModuleItem>) {
    // Extract statements for analysis
    let stmts: Vec<Option<&Stmt>> = items
        .iter()
        .map(|item| match item {
            ModuleItem::Stmt(s) => Some(s),
            _ => None,
        })
        .collect();

    let candidates = find_candidates(&stmts);
    if candidates.is_empty() {
        return;
    }

    let all_consumed: HashSet<usize> = candidates
        .iter()
        .flat_map(|c| c.consumed_indices.iter().copied())
        .collect();
    let fn_decl_map: HashMap<usize, &ClassCandidate> =
        candidates.iter().map(|c| (c.fn_decl_idx, c)).collect();

    let old = std::mem::take(items);
    for (i, item) in old.into_iter().enumerate() {
        if all_consumed.contains(&i) {
            continue;
        }
        if let Some(candidate) = fn_decl_map.get(&i) {
            let class_decl = build_class_decl(candidate, &item);
            items.push(ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl))));
        } else {
            items.push(item);
        }
    }
}

fn transform_stmts(stmts: &mut Vec<Stmt>) {
    let stmt_opts: Vec<Option<&Stmt>> = stmts.iter().map(|s| Some(s)).collect();
    let candidates = find_candidates(&stmt_opts);
    if candidates.is_empty() {
        return;
    }

    let all_consumed: HashSet<usize> = candidates
        .iter()
        .flat_map(|c| c.consumed_indices.iter().copied())
        .collect();
    let fn_decl_map: HashMap<usize, &ClassCandidate> =
        candidates.iter().map(|c| (c.fn_decl_idx, c)).collect();

    let old = std::mem::take(stmts);
    for (i, stmt) in old.into_iter().enumerate() {
        if all_consumed.contains(&i) {
            continue;
        }
        if let Some(candidate) = fn_decl_map.get(&i) {
            let class_decl = build_class_decl(candidate, &ModuleItem::Stmt(stmt));
            stmts.push(Stmt::Decl(Decl::Class(class_decl)));
        } else {
            stmts.push(stmt);
        }
    }
}

/// Find all class candidates in a list of statements.
fn find_candidates(stmts: &[Option<&Stmt>]) -> Vec<ClassCandidate> {
    let len = stmts.len();
    let get_stmt = |i: usize| stmts[i];
    // Phase 1: Find all FnDecl names and prototype method targets.
    // A function is a constructor candidate if:
    // - It has `Foo.prototype.method = function` assignments somewhere in the scope
    // - Its body references `this` OR is empty (empty constructors are common for base classes)
    let mut fn_decls: Vec<(usize, &Atom)> = Vec::new();
    for i in 0..len {
        let Some(stmt) = get_stmt(i) else { continue };
        if let Stmt::Decl(Decl::Fn(fn_decl)) = stmt {
            if has_this_reference(&fn_decl.function) || is_empty_body(&fn_decl.function) {
                fn_decls.push((i, &fn_decl.ident.sym));
            }
        }
    }

    if fn_decls.is_empty() {
        return Vec::new();
    }

    // Collect the set of names that have prototype method assignments — this is the primary trigger
    let mut names_with_proto_methods: HashSet<&Atom> = HashSet::new();
    for i in 0..len {
        let Some(stmt) = get_stmt(i) else { continue };
        let target = get_prototype_method_target(stmt).or_else(|| get_define_property_target(stmt));
        if let Some(name) = target {
            if fn_decls.iter().any(|(_, n)| n.as_ref() == name) {
                names_with_proto_methods
                    .insert(fn_decls.iter().find(|(_, n)| n.as_ref() == name).unwrap().1);
            }
        }
    }

    // Phase 2: For each candidate, collect all associated statements
    let mut candidates = Vec::new();
    let mut globally_consumed: HashSet<usize> = HashSet::new();

    for (fn_idx, name) in &fn_decls {
        if !names_with_proto_methods.contains(name) {
            continue;
        }

        let mut candidate = ClassCandidate {
            fn_decl_idx: *fn_idx,
            _name: (*name).clone(),
            super_class: None,
            super_class_name: None,
            consumed_indices: HashSet::new(),
            members: Vec::new(),
        };

        // Scan statements AFTER the fn decl for ones belonging to this class.
        // Only consuming forward avoids reordering issues with function hoisting vs class TDZ.
        for i in (*fn_idx + 1)..len {
            if globally_consumed.contains(&i) {
                continue;
            }
            let Some(stmt) = get_stmt(i) else { continue };

            // Prototype method: Foo.prototype.method = function() {}
            if let Some((method_name, fn_expr, is_static)) = extract_method_assignment(stmt, name) {
                let method = build_class_method_from_fn(method_name, fn_expr, is_static);
                candidate.members.push(ClassMember::Method(method));
                candidate.consumed_indices.insert(i);
                continue;
            }

            // Foo.prototype.constructor = Foo (redundant — skip)
            if is_prototype_constructor_assign(stmt, name) {
                candidate.consumed_indices.insert(i);
                continue;
            }

            // Foo.prototype = Object.create(Bar.prototype) — inheritance
            if let Some(super_expr) = extract_object_create_inheritance(stmt, name) {
                candidate.super_class_name = match super_expr.as_ref() {
                    Expr::Ident(id) => Some(id.sym.clone()),
                    _ => None,
                };
                candidate.super_class = Some(super_expr);
                candidate.consumed_indices.insert(i);
                continue;
            }

            // util.inherits(Foo, Bar) or inherits(Foo, Bar) — inheritance
            if let Some(super_expr) = extract_util_inherits(stmt, name) {
                candidate.super_class_name = match super_expr.as_ref() {
                    Expr::Ident(id) => Some(id.sym.clone()),
                    _ => None,
                };
                candidate.super_class = Some(super_expr);
                candidate.consumed_indices.insert(i);
                continue;
            }

            // Object.defineProperty(Foo.prototype, "name", { get/set })
            if let Some(methods) = extract_define_property(stmt, name) {
                for m in methods {
                    candidate.members.push(ClassMember::Method(m));
                }
                candidate.consumed_indices.insert(i);
                continue;
            }
        }

        // Only produce a candidate if we found at least one method
        if !candidate.members.is_empty() {
            globally_consumed.extend(&candidate.consumed_indices);
            globally_consumed.insert(*fn_idx);
            candidates.push(candidate);
        }
    }

    candidates
}

/// Build a ClassDecl from a candidate and the original FnDecl statement.
fn build_class_decl(candidate: &ClassCandidate, original_item: &ModuleItem) -> ClassDecl {
    // Extract the function from the original item
    let fn_decl = match original_item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(f))) => f,
        _ => panic!("expected FnDecl"),
    };

    let mut members = Vec::new();

    // Build constructor from the function
    let ctor = build_constructor_from_fn(&fn_decl.function, candidate.super_class_name.as_deref());
    if !is_empty_body(&fn_decl.function) {
        members.push(ClassMember::Constructor(ctor));
    }

    // Add collected methods
    members.extend(candidate.members.iter().cloned());

    ClassDecl {
        ident: fn_decl.ident.clone(),
        declare: false,
        class: Box::new(Class {
            span: DUMMY_SP,
            ctxt: Default::default(),
            decorators: vec![],
            body: members,
            super_class: candidate.super_class.clone(),
            is_abstract: false,
            type_params: None,
            super_type_params: None,
            implements: vec![],
        }),
    }
}

// ============================================================
// Statement matchers
// ============================================================

/// Get the constructor name from `Object.defineProperty(Foo.prototype, ...)`.
fn get_define_property_target(stmt: &Stmt) -> Option<&str> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(m) = callee.as_ref() else {
        return None;
    };
    let Expr::Ident(obj_id) = m.obj.as_ref() else {
        return None;
    };
    if obj_id.sym.as_ref() != "Object" {
        return None;
    }
    if !matches!(&m.prop, MemberProp::Ident(n) if n.sym.as_ref() == "defineProperty") {
        return None;
    }
    if call.args.is_empty() {
        return None;
    }
    // First arg: Foo.prototype
    let Expr::Member(target) = call.args[0].expr.as_ref() else {
        return None;
    };
    let Expr::Ident(target_obj) = target.obj.as_ref() else {
        return None;
    };
    if !matches!(&target.prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype") {
        return None;
    }
    Some(target_obj.sym.as_ref())
}

/// Get the constructor name from a `Foo.prototype.method = function` statement.
fn get_prototype_method_target(stmt: &Stmt) -> Option<&str> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(lhs)) = &assign.left else {
        return None;
    };

    // Must be Foo.prototype.something
    let Expr::Member(obj_member) = lhs.obj.as_ref() else {
        return None;
    };
    let Expr::Ident(obj_id) = obj_member.obj.as_ref() else {
        return None;
    };
    if !matches!(&obj_member.prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype") {
        return None;
    }

    // RHS must be a function expression
    if !matches!(assign.right.as_ref(), Expr::Fn(_)) {
        return None;
    }

    Some(obj_id.sym.as_ref())
}

/// Extract a method assignment: `Foo.prototype.method = function() {}` or `Foo.staticMethod = function() {}`.
/// Returns (PropName, &FnExpr, is_static).
fn extract_method_assignment<'a>(
    stmt: &'a Stmt,
    ctor_name: &Atom,
) -> Option<(PropName, &'a FnExpr, bool)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(lhs)) = &assign.left else {
        return None;
    };

    let Expr::Fn(fn_expr) = assign.right.as_ref() else {
        return None;
    };

    // Case 1: Foo.prototype.method = function() {}
    if let Expr::Member(obj_member) = lhs.obj.as_ref() {
        let Expr::Ident(obj_id) = obj_member.obj.as_ref() else {
            return None;
        };
        if &obj_id.sym != ctor_name {
            return None;
        }
        if !matches!(&obj_member.prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype") {
            return None;
        }
        let method_name = extract_prop_name(&lhs.prop)?;
        return Some((method_name, fn_expr, false));
    }

    // Case 2: Foo.staticMethod = function() {}
    if let Expr::Ident(obj_id) = lhs.obj.as_ref() {
        if &obj_id.sym != ctor_name {
            return None;
        }
        // Skip `Foo.prototype` (already handled above via member chain)
        if matches!(&lhs.prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype") {
            return None;
        }
        let method_name = extract_prop_name(&lhs.prop)?;
        return Some((method_name, fn_expr, true));
    }

    None
}

/// Check if stmt is `Foo.prototype.constructor = Foo`.
fn is_prototype_constructor_assign(stmt: &Stmt, ctor_name: &Atom) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(lhs)) = &assign.left else {
        return false;
    };

    // LHS: Foo.prototype.constructor
    let Expr::Member(obj_member) = lhs.obj.as_ref() else {
        return false;
    };
    let Expr::Ident(obj_id) = obj_member.obj.as_ref() else {
        return false;
    };
    if &obj_id.sym != ctor_name {
        return false;
    }
    if !matches!(&obj_member.prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype") {
        return false;
    }
    if !matches!(&lhs.prop, MemberProp::Ident(n) if n.sym.as_ref() == "constructor") {
        return false;
    }

    // RHS: Foo
    matches!(assign.right.as_ref(), Expr::Ident(id) if &id.sym == ctor_name)
}

/// Extract inheritance from `Foo.prototype = Object.create(Bar.prototype)`.
fn extract_object_create_inheritance(stmt: &Stmt, ctor_name: &Atom) -> Option<Box<Expr>> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(lhs)) = &assign.left else {
        return None;
    };

    // LHS: Foo.prototype
    let Expr::Ident(obj_id) = lhs.obj.as_ref() else {
        return None;
    };
    if &obj_id.sym != ctor_name {
        return None;
    }
    if !matches!(&lhs.prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype") {
        return None;
    }

    // RHS: Object.create(Bar.prototype) or Object.create(Bar.prototype, { ... })
    let Expr::Call(call) = assign.right.as_ref() else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !is_object_create(callee) {
        return None;
    }
    if call.args.is_empty() {
        return None;
    }

    // First arg should be Bar.prototype or Bar && Bar.prototype
    extract_super_from_create_arg(&call.args[0].expr)
}

/// Extract super class from `Object.create(Bar.prototype)` or `Object.create(Bar && Bar.prototype)`.
fn extract_super_from_create_arg(expr: &Expr) -> Option<Box<Expr>> {
    // Direct: Bar.prototype
    if let Expr::Member(member) = expr {
        if matches!(&member.prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype") {
            return Some(member.obj.clone());
        }
    }
    // Guarded: Bar && Bar.prototype
    if let Expr::Bin(bin) = expr {
        if bin.op == swc_core::ecma::ast::BinaryOp::LogicalAnd {
            return extract_super_from_create_arg(&bin.right);
        }
    }
    None
}

/// Extract inheritance from `util.inherits(Child, Parent)` or `inherits(Child, Parent)`.
fn extract_util_inherits(stmt: &Stmt, ctor_name: &Atom) -> Option<Box<Expr>> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };

    // Match `X.inherits(...)` or `inherits(...)`
    let is_inherits = match callee.as_ref() {
        Expr::Member(m) => {
            matches!(&m.prop, MemberProp::Ident(n) if n.sym.as_ref() == "inherits")
        }
        Expr::Ident(id) => id.sym.as_ref() == "inherits",
        _ => false,
    };
    if !is_inherits {
        return None;
    }

    if call.args.len() != 2 {
        return None;
    }

    // First arg must be the constructor name
    let Expr::Ident(first) = call.args[0].expr.as_ref() else {
        return None;
    };
    if &first.sym != ctor_name {
        return None;
    }

    // Second arg is the parent class
    Some(call.args[1].expr.clone())
}

/// Extract getters/setters from `Object.defineProperty(Foo.prototype, "name", { get/set })`.
fn extract_define_property(stmt: &Stmt, ctor_name: &Atom) -> Option<Vec<ClassMethod>> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };

    // Must be Object.defineProperty
    let Expr::Member(m) = callee.as_ref() else {
        return None;
    };
    let Expr::Ident(obj_id) = m.obj.as_ref() else {
        return None;
    };
    if obj_id.sym.as_ref() != "Object" {
        return None;
    }
    if !matches!(&m.prop, MemberProp::Ident(n) if n.sym.as_ref() == "defineProperty") {
        return None;
    }

    if call.args.len() != 3 {
        return None;
    }

    // First arg: Foo.prototype
    let Expr::Member(target) = call.args[0].expr.as_ref() else {
        return None;
    };
    let Expr::Ident(target_obj) = target.obj.as_ref() else {
        return None;
    };
    if &target_obj.sym != ctor_name {
        return None;
    }
    if !matches!(&target.prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype") {
        return None;
    }

    // Second arg: property name string
    let Expr::Lit(swc_core::ecma::ast::Lit::Str(s)) = call.args[1].expr.as_ref() else {
        return None;
    };
    let sym: Atom = s.value.as_str().unwrap_or("").into();

    // Third arg: descriptor object
    let Expr::Object(obj) = call.args[2].expr.as_ref() else {
        return None;
    };

    let mut methods = Vec::new();
    for prop in &obj.props {
        let swc_core::ecma::ast::PropOrSpread::Prop(p) = prop else {
            continue;
        };
        let swc_core::ecma::ast::Prop::KeyValue(kv) = p.as_ref() else {
            continue;
        };
        let key_name = match &kv.key {
            PropName::Ident(i) => i.sym.clone(),
            PropName::Str(s) => s.value.as_str().unwrap_or("").into(),
            _ => continue,
        };
        let kind = match key_name.as_ref() {
            "get" => MethodKind::Getter,
            "set" => MethodKind::Setter,
            _ => continue,
        };
        let Expr::Fn(fn_expr) = kv.value.as_ref() else {
            continue;
        };
        let method_key = PropName::Ident(IdentName::new(sym.clone(), DUMMY_SP));
        methods.push(build_class_method_from_fn(method_key, fn_expr, false));
        // Update kind
        if let Some(last) = methods.last_mut() {
            last.kind = kind;
        }
    }

    if methods.is_empty() {
        None
    } else {
        Some(methods)
    }
}

// ============================================================
// Helpers
// ============================================================

fn is_object_create(expr: &Expr) -> bool {
    let Expr::Member(m) = expr else { return false };
    let Expr::Ident(obj_id) = m.obj.as_ref() else {
        return false;
    };
    if obj_id.sym.as_ref() != "Object" {
        return false;
    }
    matches!(&m.prop, MemberProp::Ident(n) if n.sym.as_ref() == "create")
}

fn extract_prop_name(prop: &MemberProp) -> Option<PropName> {
    match prop {
        MemberProp::Ident(name) => {
            Some(PropName::Ident(IdentName::new(name.sym.clone(), DUMMY_SP)))
        }
        MemberProp::Computed(c) => {
            if let Expr::Lit(swc_core::ecma::ast::Lit::Str(s)) = c.expr.as_ref() {
                Some(PropName::Str(swc_core::ecma::ast::Str {
                    span: DUMMY_SP,
                    value: s.value.clone(),
                    raw: None,
                }))
            } else {
                Some(PropName::Computed(swc_core::ecma::ast::ComputedPropName {
                    span: DUMMY_SP,
                    expr: c.expr.clone(),
                }))
            }
        }
        _ => None,
    }
}

/// Check if a function body references `this`.
fn has_this_reference(func: &Function) -> bool {
    struct ThisFinder {
        found: bool,
    }
    impl Visit for ThisFinder {
        fn visit_this_expr(&mut self, _: &swc_core::ecma::ast::ThisExpr) {
            self.found = true;
        }
        // Don't descend into nested functions/arrows (they have their own `this`)
        fn visit_function(&mut self, _: &Function) {}
        fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}
    }

    let mut finder = ThisFinder { found: false };
    // Visit the body directly, not the Function node, because we override
    // visit_function to skip nested functions.
    if let Some(body) = &func.body {
        body.visit_with(&mut finder);
    }
    finder.found
}

fn is_empty_body(func: &Function) -> bool {
    match &func.body {
        None => true,
        Some(body) => body.stmts.is_empty(),
    }
}

fn build_constructor_from_fn(func: &Function, super_class_name: Option<&str>) -> Constructor {
    let mut body = func.body.clone().unwrap_or(BlockStmt {
        span: DUMMY_SP,
        ctxt: Default::default(),
        stmts: vec![],
    });

    // Rewrite `Parent.call(this, ...)` → `super(...)` if inherited
    if let Some(parent_name) = super_class_name {
        body.visit_mut_with(&mut ParentCallRewriter { parent_name });
    }

    let params: Vec<ParamOrTsParamProp> = func
        .params
        .iter()
        .map(|p| {
            ParamOrTsParamProp::Param(Param {
                span: DUMMY_SP,
                decorators: vec![],
                pat: p.pat.clone(),
            })
        })
        .collect();

    Constructor {
        span: DUMMY_SP,
        ctxt: Default::default(),
        key: PropName::Ident(IdentName::new("constructor".into(), DUMMY_SP)),
        params,
        body: Some(body),
        accessibility: None,
        is_optional: false,
    }
}

fn build_class_method_from_fn(key: PropName, fn_expr: &FnExpr, is_static: bool) -> ClassMethod {
    ClassMethod {
        span: DUMMY_SP,
        key,
        function: fn_expr.function.clone(),
        kind: MethodKind::Method,
        is_static,
        accessibility: None,
        is_abstract: false,
        is_optional: false,
        is_override: false,
    }
}

/// Rewrites `ParentName.call(this, args...)` → `super(args...)`.
struct ParentCallRewriter<'a> {
    parent_name: &'a str,
}

impl VisitMut for ParentCallRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Member(member) = callee.as_ref() else {
            return;
        };

        // Check: Parent.call
        let Expr::Ident(obj_id) = member.obj.as_ref() else {
            return;
        };
        if obj_id.sym.as_ref() != self.parent_name {
            return;
        }
        let MemberProp::Ident(prop) = &member.prop else {
            return;
        };

        match prop.sym.as_ref() {
            "call" => {
                if call.args.is_empty() {
                    return;
                }
                if !matches!(call.args[0].expr.as_ref(), Expr::This(..)) {
                    return;
                }
                let super_args: Vec<ExprOrSpread> = call.args[1..].to_vec();
                *expr = Expr::Call(CallExpr {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    callee: Callee::Super(swc_core::ecma::ast::Super { span: DUMMY_SP }),
                    args: super_args,
                    type_args: None,
                });
            }
            "apply" => {
                if call.args.len() != 2 {
                    return;
                }
                if !matches!(call.args[0].expr.as_ref(), Expr::This(..)) {
                    return;
                }
                if !matches!(call.args[1].expr.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "arguments")
                {
                    return;
                }
                let spread_arg = ExprOrSpread {
                    spread: Some(DUMMY_SP),
                    expr: call.args[1].expr.clone(),
                };
                *expr = Expr::Call(CallExpr {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    callee: Callee::Super(swc_core::ecma::ast::Super { span: DUMMY_SP }),
                    args: vec![spread_arg],
                    type_args: None,
                });
            }
            _ => {}
        }
    }

    // Don't descend into nested functions/arrows
    fn visit_mut_function(&mut self, _: &mut Function) {}
    fn visit_mut_arrow_expr(&mut self, _: &mut swc_core::ecma::ast::ArrowExpr) {}
}
