use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BlockStmt, CallExpr, Callee, Class, ClassDecl, ClassMember,
    ClassMethod, Constructor, Decl, Expr, ExprOrSpread, ExprStmt, FnExpr, Function, Ident,
    IdentName, Lit, MemberProp, MethodKind, ModuleItem, Param, ParamOrTsParamProp, Pat, PropName,
    SimpleAssignTarget, Stmt, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::utils::paren::strip_parens;

use super::decl_utils::has_duplicate_param_names;
use super::helper_matcher::{binding_key, BindingKey};

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
    /// Index of the constructor declaration in the statement list.
    fn_decl_idx: usize,
    /// Whether the constructor came from a hoisted declaration or a variable initializer.
    constructor_kind: ConstructorKind,
    /// The constructor binding (e.g., `Foo` plus its resolved syntax context).
    binding: BindingKey,
    /// Super class expression, if inheritance is detected.
    super_class: Option<Box<Expr>>,
    /// Super class name for `Parent.call(this, ...)` → `super(...)` rewriting.
    super_class_binding: Option<BindingKey>,
    /// Indices of statements consumed by this class (prototype methods, inheritance, etc.).
    consumed_indices: HashSet<usize>,
    /// Statements before the fn decl that reference the function name (in order).
    /// Relocated to after the class declaration to avoid TDZ.
    pre_ref_indices: Vec<usize>,
    /// className value extracted from chained inheritance (e.g., "Root").
    /// Emitted as `Foo.className = "Root"` after the class declaration.
    class_name_value: Option<Atom>,
    /// Collected class members.
    members: Vec<ClassMember>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConstructorKind {
    FunctionDeclaration,
    VariableFunction,
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

    let candidates = find_candidates(&stmts, true);
    if candidates.is_empty() {
        return;
    }

    let mut all_consumed: HashSet<usize> = candidates
        .iter()
        .flat_map(|c| c.consumed_indices.iter().copied())
        .collect();
    // Pre-ref statements are also skipped at their original position (relocated after class).
    let all_pre_refs: HashSet<usize> = candidates
        .iter()
        .flat_map(|c| c.pre_ref_indices.iter().copied())
        .collect();
    all_consumed.extend(&all_pre_refs);
    let fn_decl_map: HashMap<usize, &ClassCandidate> =
        candidates.iter().map(|c| (c.fn_decl_idx, c)).collect();

    let old: Vec<ModuleItem> = std::mem::take(items);
    for (i, item) in old.iter().enumerate() {
        if all_consumed.contains(&i) {
            continue;
        }
        if let Some(candidate) = fn_decl_map.get(&i) {
            if let ModuleItem::Stmt(stmt) = item {
                if let Some(class_decl) = build_class_decl(candidate, stmt) {
                    items.push(ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl))));
                    for &pre_idx in &candidate.pre_ref_indices {
                        items.push(old[pre_idx].clone());
                    }
                    if let Some(cn) = &candidate.class_name_value {
                        items.push(ModuleItem::Stmt(make_class_name_stmt(
                            &candidate.binding,
                            cn,
                        )));
                    }
                    continue;
                }
            }
            debug_assert_candidate_points_to_function_decl(item);
            items.push(item.clone());
        } else {
            items.push(item.clone());
        }
    }
}

fn transform_stmts(stmts: &mut Vec<Stmt>) {
    let stmt_opts: Vec<Option<&Stmt>> = stmts.iter().map(Some).collect();
    let candidates = find_candidates(&stmt_opts, false);
    if candidates.is_empty() {
        return;
    }

    let mut all_consumed: HashSet<usize> = candidates
        .iter()
        .flat_map(|c| c.consumed_indices.iter().copied())
        .collect();
    let all_pre_refs: HashSet<usize> = candidates
        .iter()
        .flat_map(|c| c.pre_ref_indices.iter().copied())
        .collect();
    all_consumed.extend(&all_pre_refs);
    let fn_decl_map: HashMap<usize, &ClassCandidate> =
        candidates.iter().map(|c| (c.fn_decl_idx, c)).collect();

    let old: Vec<Stmt> = std::mem::take(stmts);
    for (i, stmt) in old.iter().enumerate() {
        if all_consumed.contains(&i) {
            continue;
        }
        if let Some(candidate) = fn_decl_map.get(&i) {
            if let Some(class_decl) = build_class_decl(candidate, stmt) {
                stmts.push(Stmt::Decl(Decl::Class(class_decl)));
                for &pre_idx in &candidate.pre_ref_indices {
                    stmts.push(old[pre_idx].clone());
                }
                if let Some(cn) = &candidate.class_name_value {
                    stmts.push(make_class_name_stmt(&candidate.binding, cn));
                }
            } else {
                debug_assert_stmt_is_function_decl(stmt);
                stmts.push(stmt.clone());
            }
        } else {
            stmts.push(stmt.clone());
        }
    }
}

fn debug_assert_candidate_points_to_function_decl(item: &ModuleItem) {
    debug_assert!(
        matches!(item, ModuleItem::Stmt(stmt) if extract_constructor(stmt, true).is_some()),
        "class candidate did not point to a supported constructor declaration"
    );
}

fn debug_assert_stmt_is_function_decl(stmt: &Stmt) {
    debug_assert!(
        extract_constructor(stmt, true).is_some(),
        "class candidate did not point to a supported constructor declaration"
    );
}

fn extract_constructor(
    stmt: &Stmt,
    allow_module_var: bool,
) -> Option<(&Ident, &Function, ConstructorKind)> {
    match stmt {
        Stmt::Decl(Decl::Fn(fn_decl)) => Some((
            &fn_decl.ident,
            &fn_decl.function,
            ConstructorKind::FunctionDeclaration,
        )),
        Stmt::Decl(Decl::Var(var_decl)) => {
            if var_decl.kind != VarDeclKind::Const
                && !(allow_module_var && var_decl.kind == VarDeclKind::Var)
            {
                return None;
            }
            let [declarator] = var_decl.decls.as_slice() else {
                return None;
            };
            let Pat::Ident(binding) = &declarator.name else {
                return None;
            };
            let Expr::Fn(fn_expr) = strip_parens(declarator.init.as_deref()?) else {
                return None;
            };
            // A named function expression has an additional inner binding whose
            // recursion semantics do not map directly to a class declaration.
            if fn_expr.ident.is_some() {
                return None;
            }
            Some((
                &binding.id,
                &fn_expr.function,
                ConstructorKind::VariableFunction,
            ))
        }
        _ => None,
    }
}

/// Find all class candidates in a list of statements.
fn find_candidates(stmts: &[Option<&Stmt>], allow_module_var: bool) -> Vec<ClassCandidate> {
    let len = stmts.len();
    let get_stmt = |i: usize| stmts[i];
    // Phase 1: Find function declarations and single-declarator anonymous
    // function initializers (`var Foo = function() {}`). Closure Compiler's
    // ES5 output uses the latter for classes.
    // A function is a constructor candidate if:
    // - It has `Foo.prototype.method = function` assignments somewhere in the scope
    // - Its body references `this` OR is empty (empty constructors are common for base classes)
    let mut fn_decls: Vec<(usize, BindingKey, ConstructorKind)> = Vec::new();
    for i in 0..len {
        let Some(stmt) = get_stmt(i) else { continue };
        let Some((ident, function, kind)) = extract_constructor(stmt, allow_module_var) else {
            continue;
        };
        if !has_duplicate_param_names(&function.params)
            && (has_this_reference(function) || is_empty_body(function))
        {
            fn_decls.push((i, binding_key(ident), kind));
        }
    }

    if fn_decls.is_empty() {
        return Vec::new();
    }

    // Collect the set of names that have prototype method assignments — this is the primary trigger
    let mut names_with_proto_methods: HashSet<BindingKey> = HashSet::new();
    for i in 0..len {
        let Some(stmt) = get_stmt(i) else { continue };
        let target = get_prototype_method_target(stmt).or_else(|| get_define_property_target(stmt));
        if let Some(binding) = target {
            if fn_decls
                .iter()
                .any(|(_, candidate, _)| candidate == &binding)
            {
                names_with_proto_methods.insert(binding);
            }
        }
    }

    // Phase 2: For each candidate, collect all associated statements
    let mut candidates = Vec::new();
    let mut globally_consumed: HashSet<usize> = HashSet::new();

    for (fn_idx, binding, constructor_kind) in &fn_decls {
        if !names_with_proto_methods.contains(binding) {
            continue;
        }

        // Collect pre-reference statements. Each must be either:
        // - A safe-to-relocate pattern (export alias, static string prop)
        // - A chained inheritance expression (consumed, super class extracted)
        // Any other reference to the name → skip candidate entirely.
        let mut pre_ref_indices: Vec<usize> = Vec::new();
        let mut pre_consumed_indices: HashSet<usize> = HashSet::new();
        let mut pre_super_class: Option<Box<Expr>> = None;
        let mut pre_super_class_name: Option<BindingKey> = None;
        let mut pre_class_name_value: Option<Atom> = None;
        let mut has_unsafe_pre_ref = false;

        for (i, slot) in stmts.iter().enumerate().take(*fn_idx) {
            if globally_consumed.contains(&i) {
                continue;
            }
            let Some(stmt) = slot else { continue };
            if !references_binding(
                stmt,
                binding,
                *constructor_kind == ConstructorKind::VariableFunction,
            ) {
                continue;
            }

            // Function-expression variables are not hoisted with an initialized
            // value. Relocating a pre-reference would change a TDZ/undefined
            // access into a valid class reference, so leave the whole shape alone.
            if *constructor_kind == ConstructorKind::VariableFunction {
                has_unsafe_pre_ref = true;
                break;
            }

            // Try chained inheritance pattern first:
            // ((Foo.prototype = Object.create(Bar.prototype)).constructor = Foo).className = "X"
            if let Some((sc, sn, cn)) = extract_chained_inheritance(stmt, binding) {
                pre_super_class = Some(sc);
                pre_super_class_name = sn;
                if let Some(v) = cn {
                    pre_class_name_value = Some(v);
                }
                pre_consumed_indices.insert(i);
                continue;
            }

            if !is_safe_to_relocate(stmt, binding) {
                has_unsafe_pre_ref = true;
                break;
            }
            pre_ref_indices.push(i);
        }

        if has_unsafe_pre_ref {
            continue;
        }

        let mut candidate = ClassCandidate {
            fn_decl_idx: *fn_idx,
            constructor_kind: *constructor_kind,
            binding: binding.clone(),
            super_class: pre_super_class,
            super_class_binding: pre_super_class_name,
            consumed_indices: pre_consumed_indices,
            pre_ref_indices,
            class_name_value: pre_class_name_value,
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
            if let Some((method_name, fn_expr, is_static)) =
                extract_method_assignment(stmt, binding)
            {
                let method = build_class_method_from_fn(method_name, fn_expr, is_static);
                candidate.members.push(ClassMember::Method(method));
                candidate.consumed_indices.insert(i);
                continue;
            }

            // Foo.prototype.constructor = Foo (redundant — skip)
            if is_prototype_constructor_assign(stmt, binding) {
                candidate.consumed_indices.insert(i);
                continue;
            }

            // Foo.prototype = Object.create(Bar.prototype) — inheritance
            // Skip if already found via chained pre-reference.
            if candidate.super_class.is_none() {
                if let Some(super_expr) = extract_object_create_inheritance(stmt, binding) {
                    candidate.super_class_binding = match super_expr.as_ref() {
                        Expr::Ident(id) => Some(binding_key(id)),
                        _ => None,
                    };
                    candidate.super_class = Some(super_expr);
                    candidate.consumed_indices.insert(i);
                    continue;
                }
            }

            // util.inherits(Foo, Bar) or inherits(Foo, Bar) — inheritance
            if candidate.super_class.is_none() {
                if let Some(super_expr) = extract_util_inherits(stmt, binding) {
                    candidate.super_class_binding = match super_expr.as_ref() {
                        Expr::Ident(id) => Some(binding_key(id)),
                        _ => None,
                    };
                    candidate.super_class = Some(super_expr);
                    candidate.consumed_indices.insert(i);
                    continue;
                }
            }

            // Object.defineProperty(Foo.prototype, "name", { get/set })
            if let Some(methods) = extract_define_property(stmt, binding) {
                for m in methods {
                    candidate.members.push(ClassMember::Method(m));
                }
                candidate.consumed_indices.insert(i);
                continue;
            }
        }

        // Moving a later prototype assignment into the class also moves it
        // ahead of every preserved statement between the constructor and that
        // assignment. An unrecognized helper call involving the constructor
        // may replace its prototype (for example `tm.inherit(Child, Base)`), so
        // preserve the original ordering. Ordinary reads and `new Child()` are
        // safe and are intentionally not blocked.
        let has_interleaved_constructor_call = *constructor_kind
            == ConstructorKind::VariableFunction
            && candidate
                .consumed_indices
                .iter()
                .copied()
                .max()
                .is_some_and(|last_consumed| {
                    ((*fn_idx + 1)..last_consumed).any(|i| {
                        !candidate.consumed_indices.contains(&i)
                            && get_stmt(i)
                                .is_some_and(|stmt| is_call_referencing_binding(stmt, binding))
                    })
                });
        if has_interleaved_constructor_call {
            continue;
        }

        // A retained `Foo.prototype = <expr>` anywhere in the scope (including
        // inside a recovered method or closure that may run at any time) replaces the
        // whole prototype object. Recovering a class would bake the collected
        // methods into the class prototype instead of the replacement object,
        // and the leftover assignment would target a class's non-writable
        // `prototype` (a strict-mode TypeError). This hazard is independent
        // of the constructor's declaration shape.
        let has_retained_prototype_replacement = (0..len).any(|i| {
            get_stmt(i).is_some_and(|stmt| {
                // These exact whole-prototype writes are consumed as recognized
                // inheritance and do not remain in the recovered class.
                let consumed_inheritance = candidate.consumed_indices.contains(&i)
                    && (extract_chained_inheritance(stmt, binding).is_some()
                        || extract_object_create_inheritance(stmt, binding).is_some());
                !consumed_inheritance && contains_prototype_replacement(stmt, binding)
            })
        });
        if has_retained_prototype_replacement {
            continue;
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

fn is_call_referencing_binding(stmt: &Stmt, binding: &BindingKey) -> bool {
    matches!(stmt, Stmt::Expr(expr_stmt) if matches!(expr_stmt.expr.as_ref(), Expr::Call(_)))
        && references_binding(stmt, binding, true)
}

/// Check if a statement contains an assignment that replaces the whole
/// prototype object (`Foo.prototype = <expr>`), in any position — top-level,
/// chained, or inside a nested function that may run at any time.
fn contains_prototype_replacement(stmt: &Stmt, ctor_binding: &BindingKey) -> bool {
    struct ReplacementFinder<'a> {
        binding: &'a BindingKey,
        found: bool,
    }
    impl Visit for ReplacementFinder<'_> {
        fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
            if let AssignTarget::Simple(SimpleAssignTarget::Member(lhs)) = &assign.left {
                if let Expr::Ident(obj) = lhs.obj.as_ref() {
                    if binding_key(obj) == *self.binding
                        && matches!(&lhs.prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype")
                    {
                        self.found = true;
                    }
                }
            }
            assign.visit_children_with(self);
        }
    }

    let mut finder = ReplacementFinder {
        binding: ctor_binding,
        found: false,
    };
    stmt.visit_with(&mut finder);
    finder.found
}

/// Check if a statement references a binding. Variable-function candidates
/// include nested functions because an earlier closure can observe `var`
/// hoisting when it is invoked before the initializer.
fn references_binding(stmt: &Stmt, binding: &BindingKey, include_nested: bool) -> bool {
    struct BindingRefFinder<'a> {
        binding: &'a BindingKey,
        include_nested: bool,
        found: bool,
    }
    impl Visit for BindingRefFinder<'_> {
        fn visit_ident(&mut self, id: &Ident) {
            if binding_key(id) == *self.binding {
                self.found = true;
            }
        }

        fn visit_function(&mut self, function: &Function) {
            if self.include_nested {
                function.visit_children_with(self);
            }
        }

        fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
            if self.include_nested {
                arrow.visit_children_with(self);
            }
        }
    }

    let mut finder = BindingRefFinder {
        binding,
        include_nested,
        found: false,
    };
    stmt.visit_with(&mut finder);
    finder.found
}

/// Check if a pre-reference statement is safe to relocate after the class.
/// Exactly three patterns are allowed:
///   1. `<ident>.exports = Foo`  (e.g., `module.exports = Foo`, `fn4.exports = Foo`)
///   2. `exports.<ident> = Foo`  (e.g., `exports.default = Foo`)
///   3. `Foo.<ident> = <expr>` (static property/method, e.g., `Foo.className = "X"`,
///      `Foo.fromJSON = (K, _) => ...`)
fn is_safe_to_relocate(stmt: &Stmt, binding: &BindingKey) -> bool {
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
    let Expr::Ident(obj) = lhs.obj.as_ref() else {
        return false;
    };
    let MemberProp::Ident(prop) = &lhs.prop else {
        return false;
    };

    // Pattern 1: <ident>.exports = Foo
    if prop.sym.as_ref() == "exports" && binding_key(obj) != *binding {
        return matches!(assign.right.as_ref(), Expr::Ident(id) if binding_key(id) == *binding);
    }

    // Pattern 2: exports.<ident> = Foo
    if obj.sym.as_ref() == "exports" {
        return matches!(assign.right.as_ref(), Expr::Ident(id) if binding_key(id) == *binding);
    }

    // Pattern 3: Foo.<ident> = <expr> (static property/method assignment).
    // `Foo.prototype = <expr>` is excluded: relocating a prototype
    // replacement after the class would strand the recovered methods on the
    // class prototype (see contains_prototype_replacement).
    if binding_key(obj) == *binding {
        return prop.sym.as_ref() != "prototype";
    }

    false
}

fn unwrap_paren(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(p) => unwrap_paren(&p.expr),
        _ => expr,
    }
}

/// Recognize protobuf.js-style chained inheritance expressions before the fn decl.
/// This is NOT a standard transpiler pattern — it's specific to protobuf.js codegen:
///   `((Foo.prototype = Object.create(Bar.prototype)).constructor = Foo).className = "X"`
///   `(Foo.prototype = Object.create(Bar.prototype)).constructor = Foo`
/// Returns (super_class, super_class_name, class_name_value).
fn extract_chained_inheritance(
    stmt: &Stmt,
    ctor_binding: &BindingKey,
) -> Option<(Box<Expr>, Option<BindingKey>, Option<Atom>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    extract_chained_inheritance_expr(expr, ctor_binding)
}

fn extract_chained_inheritance_expr(
    expr: &Expr,
    ctor_binding: &BindingKey,
) -> Option<(Box<Expr>, Option<BindingKey>, Option<Atom>)> {
    let expr = unwrap_paren(expr);
    let Expr::Assign(assign) = expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(lhs)) = &assign.left else {
        return None;
    };
    let MemberProp::Ident(prop) = &lhs.prop else {
        return None;
    };

    match prop.sym.as_ref() {
        "className" => {
            let Expr::Lit(swc_core::ecma::ast::Lit::Str(s)) = assign.right.as_ref() else {
                return None;
            };
            let (sc, sn, _) =
                extract_chained_inheritance_expr(unwrap_paren(lhs.obj.as_ref()), ctor_binding)?;
            Some((sc, sn, Some(Atom::from(s.value.as_str().unwrap_or("")))))
        }
        "constructor" => {
            let Expr::Ident(rhs) = assign.right.as_ref() else {
                return None;
            };
            if binding_key(rhs) != *ctor_binding {
                return None;
            }
            extract_chained_inheritance_expr(unwrap_paren(lhs.obj.as_ref()), ctor_binding)
        }
        "prototype" => {
            let Expr::Ident(obj) = lhs.obj.as_ref() else {
                return None;
            };
            if binding_key(obj) != *ctor_binding {
                return None;
            }
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
            let super_class = extract_super_from_create_arg(&call.args[0].expr)?;
            let super_name = match super_class.as_ref() {
                Expr::Ident(id) => Some(binding_key(id)),
                _ => None,
            };
            Some((super_class, super_name, None))
        }
        _ => None,
    }
}

/// Synthesize `Foo.className = "X"` statement.
fn make_class_name_stmt(binding: &BindingKey, class_name_value: &Atom) -> Stmt {
    use swc_core::ecma::ast::{AssignExpr, Ident, MemberExpr, Str};
    Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Ident(Ident::new(
                    binding.0.clone(),
                    DUMMY_SP,
                    binding.1,
                ))),
                prop: MemberProp::Ident(IdentName::new("className".into(), DUMMY_SP)),
            })),
            right: Box::new(Expr::Lit(swc_core::ecma::ast::Lit::Str(Str {
                span: DUMMY_SP,
                value: class_name_value.as_str().into(),
                raw: None,
            }))),
        })),
    })
}

/// Build a ClassDecl from a candidate and the original FnDecl statement.
fn build_class_decl(candidate: &ClassCandidate, original_stmt: &Stmt) -> Option<ClassDecl> {
    let (ident, function, kind) = extract_constructor(original_stmt, true)?;
    if kind != candidate.constructor_kind {
        return None;
    }

    let mut members = Vec::new();

    // Build constructor from the function
    let ctor = build_constructor_from_fn(function, candidate.super_class_binding.as_ref());
    if !is_empty_body(function) {
        members.push(ClassMember::Constructor(ctor));
    }

    // Add collected methods
    members.extend(candidate.members.iter().cloned());

    let class_span = if function.span.lo.0 != 0 {
        function.span
    } else {
        DUMMY_SP
    };
    Some(ClassDecl {
        ident: ident.clone(),
        declare: false,
        class: Box::new(Class {
            span: class_span,
            ctxt: Default::default(),
            decorators: vec![],
            body: members,
            super_class: candidate.super_class.clone(),
            is_abstract: false,
            type_params: None,
            super_type_params: None,
            implements: vec![],
        }),
    })
}

// ============================================================
// Statement matchers
// ============================================================

/// Get the constructor name from `Object.defineProperty(Foo.prototype, ...)`.
fn get_define_property_target(stmt: &Stmt) -> Option<BindingKey> {
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
    Some(binding_key(target_obj))
}

/// Get the constructor name from a `Foo.prototype.method = function` statement.
fn get_prototype_method_target(stmt: &Stmt) -> Option<BindingKey> {
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

    Some(binding_key(obj_id))
}

/// Extract a method assignment: `Foo.prototype.method = function() {}` or `Foo.staticMethod = function() {}`.
/// Returns (PropName, &FnExpr, is_static).
fn extract_method_assignment<'a>(
    stmt: &'a Stmt,
    ctor_binding: &BindingKey,
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
    if has_duplicate_param_names(&fn_expr.function.params) {
        return None;
    }

    // Case 1: Foo.prototype.method = function() {}
    if let Expr::Member(obj_member) = lhs.obj.as_ref() {
        let Expr::Ident(obj_id) = obj_member.obj.as_ref() else {
            return None;
        };
        if binding_key(obj_id) != *ctor_binding {
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
        if binding_key(obj_id) != *ctor_binding {
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
fn is_prototype_constructor_assign(stmt: &Stmt, ctor_binding: &BindingKey) -> bool {
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
    if binding_key(obj_id) != *ctor_binding {
        return false;
    }
    if !matches!(&obj_member.prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype") {
        return false;
    }
    if !matches!(&lhs.prop, MemberProp::Ident(n) if n.sym.as_ref() == "constructor") {
        return false;
    }

    // RHS: Foo
    matches!(assign.right.as_ref(), Expr::Ident(id) if binding_key(id) == *ctor_binding)
}

/// Extract inheritance from `Foo.prototype = Object.create(Bar.prototype)`.
fn extract_object_create_inheritance(stmt: &Stmt, ctor_binding: &BindingKey) -> Option<Box<Expr>> {
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
    if binding_key(obj_id) != *ctor_binding {
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
fn extract_util_inherits(stmt: &Stmt, ctor_binding: &BindingKey) -> Option<Box<Expr>> {
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
    if binding_key(first) != *ctor_binding {
        return None;
    }

    // Second arg is the parent class
    Some(call.args[1].expr.clone())
}

/// Extract methods/accessors from `Object.defineProperty(Foo.prototype, "name", descriptor)`.
fn extract_define_property(stmt: &Stmt, ctor_binding: &BindingKey) -> Option<Vec<ClassMethod>> {
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
    if binding_key(target_obj) != *ctor_binding {
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
    let value_fn = descriptor_value_method_fn(obj);
    if value_fn.is_some_and(|fn_expr| has_duplicate_param_names(&fn_expr.function.params)) {
        return None;
    }

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
        if has_duplicate_param_names(&fn_expr.function.params) {
            return None;
        }
        let method_key = PropName::Ident(IdentName::new(sym.clone(), DUMMY_SP));
        methods.push(build_class_method_from_fn(method_key, fn_expr, false));
        // Update kind
        if let Some(last) = methods.last_mut() {
            last.kind = kind;
        }
    }

    if let Some(fn_expr) = value_fn {
        let method_key = PropName::Ident(IdentName::new(sym, DUMMY_SP));
        methods.push(build_class_method_from_fn(method_key, fn_expr, false));
    }

    if methods.is_empty() {
        None
    } else {
        Some(methods)
    }
}

fn descriptor_value_method_fn(obj: &swc_core::ecma::ast::ObjectLit) -> Option<&FnExpr> {
    let mut value_fn = None;
    let mut writable = false;
    let mut configurable = false;

    for prop in &obj.props {
        let swc_core::ecma::ast::PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let swc_core::ecma::ast::Prop::KeyValue(kv) = prop.as_ref() else {
            return None;
        };
        let key_name = prop_name_atom(&kv.key)?;
        match key_name.as_ref() {
            "value" => match strip_parens(&kv.value) {
                Expr::Fn(fn_expr) => value_fn = Some(fn_expr),
                _ => return None,
            },
            "writable" => {
                if !is_bool_literal(&kv.value, true) {
                    return None;
                }
                writable = true;
            }
            "configurable" => {
                if !is_bool_literal(&kv.value, true) {
                    return None;
                }
                configurable = true;
            }
            "enumerable" => {
                if !is_bool_literal(&kv.value, false) {
                    return None;
                }
            }
            _ => return None,
        }
    }

    if writable && configurable {
        value_fn
    } else {
        None
    }
}

fn prop_name_atom(prop: &PropName) -> Option<Atom> {
    match prop {
        PropName::Ident(ident) => Some(ident.sym.clone()),
        PropName::Str(str_lit) => Some(str_lit.value.as_str()?.into()),
        _ => None,
    }
}

fn is_bool_literal(expr: &Expr, value: bool) -> bool {
    matches!(strip_parens(expr), Expr::Lit(Lit::Bool(bool_lit)) if bool_lit.value == value)
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

fn build_constructor_from_fn(
    func: &Function,
    super_class_binding: Option<&BindingKey>,
) -> Constructor {
    let mut body = func.body.clone().unwrap_or(BlockStmt {
        span: DUMMY_SP,
        ctxt: Default::default(),
        stmts: vec![],
    });

    // Rewrite `Parent.call(this, ...)` → `super(...)` if inherited
    if let Some(parent_binding) = super_class_binding {
        body.visit_mut_with(&mut ParentCallRewriter { parent_binding });
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
    parent_binding: &'a BindingKey,
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
        if binding_key(obj_id) != *self.parent_binding {
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
                let original_span = call.span;
                *expr = Expr::Call(CallExpr {
                    span: if original_span.lo.0 != 0 {
                        original_span
                    } else {
                        DUMMY_SP
                    },
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
                let original_span = call.span;
                *expr = Expr::Call(CallExpr {
                    span: if original_span.lo.0 != 0 {
                        original_span
                    } else {
                        DUMMY_SP
                    },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_class_decl_returns_none_for_non_function_statement() {
        let candidate = ClassCandidate {
            fn_decl_idx: 0,
            constructor_kind: ConstructorKind::FunctionDeclaration,
            binding: ("Foo".into(), Default::default()),
            super_class: None,
            super_class_binding: None,
            consumed_indices: HashSet::new(),
            pre_ref_indices: Vec::new(),
            class_name_value: None,
            members: Vec::new(),
        };
        let stmt = Stmt::Empty(swc_core::ecma::ast::EmptyStmt { span: DUMMY_SP });

        assert!(build_class_decl(&candidate, &stmt).is_none());
    }
}
