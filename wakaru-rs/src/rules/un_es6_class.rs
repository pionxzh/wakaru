use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, BindingIdent, BlockStmt, BlockStmtOrExpr, CallExpr, Callee,
    Class, ClassDecl, ClassMember, ClassMethod, ComputedPropName, Constructor, Decl, Expr,
    ExprOrSpread, ExprStmt, FnExpr, Function, Ident, IdentName, MemberExpr, MemberProp, MethodKind,
    ModuleItem, Param, ParamOrTsParamProp, Pat, PropName, SeqExpr, SimpleAssignTarget, Stmt, VarDecl,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnEs6Class;

impl VisitMut for UnEs6Class {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        // Pre-scan for inherits helpers at module level BEFORE visiting children,
        // so nested scopes (function bodies) can also detect custom inherits calls.
        let inherits_helpers = collect_inherits_helpers_from_items(items);

        let mut inner = UnEs6ClassInner {
            inherits_helpers,
        };
        items.visit_mut_with(&mut inner);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        // Non-module context: scan local scope for inherits helpers
        let inherits_helpers = collect_inherits_helpers_from_stmts(stmts);

        let mut inner = UnEs6ClassInner {
            inherits_helpers,
        };
        stmts.visit_mut_with(&mut inner);
    }
}

/// Inner visitor that carries the inherits helpers set through all scopes.
struct UnEs6ClassInner {
    inherits_helpers: HashSet<Atom>,
}

impl VisitMut for UnEs6ClassInner {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        let old = std::mem::take(stmts);
        for stmt in old {
            match stmt {
                Stmt::Decl(Decl::Var(ref var_decl)) => {
                    if let Some(class_decl) = try_iife_to_class(var_decl, &self.inherits_helpers) {
                        stmts.push(Stmt::Decl(Decl::Class(class_decl)));
                    } else {
                        stmts.push(stmt);
                    }
                }
                other => stmts.push(other),
            }
        }
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let old = std::mem::take(items);
        for item in old {
            match item {
                ModuleItem::Stmt(Stmt::Decl(Decl::Var(ref var_decl))) => {
                    if let Some(class_decl) = try_iife_to_class(var_decl, &self.inherits_helpers) {
                        items.push(ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl))));
                    } else {
                        items.push(item);
                    }
                }
                other => items.push(other),
            }
        }
    }
}

/// Collect names of functions that match the `_inherits` body shape from statements.
fn collect_inherits_helpers_from_stmts(stmts: &[Stmt]) -> HashSet<Atom> {
    let mut helpers = HashSet::new();
    for stmt in stmts {
        if let Stmt::Decl(Decl::Fn(fn_decl)) = stmt {
            if is_inherits_fn(&fn_decl.function) {
                helpers.insert(fn_decl.ident.sym.clone());
            }
        }
    }
    helpers
}

/// Collect names of functions that match the `_inherits` body shape from module items.
fn collect_inherits_helpers_from_items(items: &[ModuleItem]) -> HashSet<Atom> {
    let mut helpers = HashSet::new();
    for item in items {
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) = item {
            if is_inherits_fn(&fn_decl.function) {
                helpers.insert(fn_decl.ident.sym.clone());
            }
        }
    }
    helpers
}

/// Check if a function body matches the `_inherits` pattern:
/// ```js
/// function _inherits(e, t) {
///     e.prototype = Object.create(t.prototype);
///     e.prototype.constructor = e;
///     // optional: setPrototypeOf / __proto__
/// }
/// ```
fn is_inherits_fn(func: &Function) -> bool {
    if func.params.len() != 2 {
        return false;
    }
    let Pat::Ident(param1) = &func.params[0].pat else { return false };
    let body = match &func.body {
        Some(b) => b,
        None => return false,
    };
    // Must contain `param1.prototype = Object.create(...)` — the key signal.
    // Just checking for any Object.create is too loose (would match utility functions).
    if body.stmts.len() > 5 {
        return false;
    }
    body.stmts.iter().any(|s| is_prototype_assign_object_create(s, &param1.id.sym))
}

/// Check if a statement is `param.prototype = Object.create(...)`.
fn is_prototype_assign_object_create(stmt: &Stmt, param_name: &Atom) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else { return false };
    let Expr::Assign(assign) = expr.as_ref() else { return false };
    if assign.op != AssignOp::Assign {
        return false;
    }
    // LHS must be `param.prototype`
    let AssignTarget::Simple(SimpleAssignTarget::Member(lhs)) = &assign.left else { return false };
    let Expr::Ident(obj) = lhs.obj.as_ref() else { return false };
    if &obj.sym != param_name {
        return false;
    }
    if !matches!(&lhs.prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype") {
        return false;
    }
    // RHS must contain Object.create(...)
    let rhs = strip_parens(&assign.right);
    let Expr::Call(call) = rhs else { return false };
    let Callee::Expr(callee) = &call.callee else { return false };
    is_object_create_callee(callee)
}

// ============================================================
// Core transformation
// ============================================================

/// Attempt to convert a `var Foo = (function(...) { ... }(...))` pattern into a ClassDecl.
/// Returns None if the pattern doesn't match.
fn try_iife_to_class(var: &VarDecl, inherits_helpers: &HashSet<Atom>) -> Option<ClassDecl> {
    // Must be a single declarator
    if var.decls.len() != 1 {
        return None;
    }
    let declarator = &var.decls[0];

    // Name must be a plain identifier
    let Pat::Ident(BindingIdent { id: class_name, .. }) = &declarator.name else {
        return None;
    };

    // Must have an initializer
    let init = declarator.init.as_ref()?;

    // The init must be an IIFE call expression (possibly paren-wrapped)
    let call = extract_iife_call(init)?;

    // Callee must be a function or arrow expression (possibly paren-wrapped)
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let callee_inner = strip_parens(callee_expr);

    // Extract params and body from either FnExpr or ArrowExpr
    let (param_pats, body_stmts): (Vec<&Pat>, &[Stmt]) = match callee_inner {
        Expr::Fn(fn_expr) => {
            let body = fn_expr.function.body.as_ref()?;
            let pats: Vec<&Pat> = fn_expr.function.params.iter().map(|p| &p.pat).collect();
            (pats, &body.stmts)
        }
        Expr::Arrow(arrow) => {
            let BlockStmtOrExpr::BlockStmt(block) = &*arrow.body else {
                return None;
            };
            let pats: Vec<&Pat> = arrow.params.iter().collect();
            (pats, &block.stmts)
        }
        _ => return None,
    };

    // The IIFE takes 0 args (no extends) or 1 arg (extends from _super)
    let (super_class, inner_param): (Option<Box<Expr>>, Option<Atom>) = match call.args.len() {
        0 => (None, None),
        1 => {
            if param_pats.len() != 1 {
                return None;
            }
            let Pat::Ident(BindingIdent { id: param_id, .. }) = param_pats[0] else {
                return None;
            };
            let super_expr = call.args[0].expr.clone();
            (Some(super_expr), Some(param_id.sym.clone()))
        }
        _ => return None,
    };

    // 0-arg IIFE must have 0 params as well
    if call.args.is_empty() && !param_pats.is_empty() {
        return None;
    }

    let class_body = parse_class_body(body_stmts, &class_name.sym, inner_param.as_deref(), inherits_helpers)?;

    Some(ClassDecl {
        ident: class_name.clone(),
        declare: false,
        class: Box::new(Class {
            span: DUMMY_SP,
            ctxt: Default::default(),
            decorators: vec![],
            body: class_body,
            super_class,
            is_abstract: false,
            type_params: None,
            super_type_params: None,
            implements: vec![],
        }),
    })
}

// ============================================================
// IIFE structure helpers
// ============================================================

/// Strip parentheses and try to extract the inner CallExpr that represents the IIFE invocation.
///
/// Handles both function and arrow IIFEs:
///   `(function() { ... }())` / `(function() { ... })()`
///   `((e) => { ... })(arg)`
fn extract_iife_call(expr: &Expr) -> Option<&CallExpr> {
    let stripped = strip_parens(expr);
    match stripped {
        Expr::Call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            let inner = strip_parens(callee);
            if matches!(inner, Expr::Fn(_) | Expr::Arrow(_)) {
                Some(call)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn extract_fn_expr(expr: &Expr) -> Option<&FnExpr> {
    match expr {
        Expr::Fn(fn_expr) => Some(fn_expr),
        Expr::Paren(paren) => extract_fn_expr(&paren.expr),
        _ => None,
    }
}

fn strip_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_parens(&paren.expr),
        _ => expr,
    }
}

// ============================================================
// Class body parsing
// ============================================================

/// Parse the statements inside the IIFE body and collect class members.
///
/// `class_name` — the outer variable name (e.g. `"Foo"`)
/// `super_param` — the IIFE parameter name that represents `_super` (if inheriting)
///
/// Returns None if any statement is unrecognised (conservative — no false positives).
fn parse_class_body(
    stmts: &[Stmt],
    class_name: &str,
    super_param: Option<&str>,
    inherits_helpers: &HashSet<Atom>,
) -> Option<Vec<ClassMember>> {
    // The first real statement should define the constructor function.
    // We need to identify the inner constructor function name (often mangled, e.g. `t`).
    let inner_ctor_name = find_inner_constructor_name(stmts)?;

    let mut members: Vec<ClassMember> = Vec::new();
    // Tracks whether we've seen and handled the `__extends` / `_inherits` call
    let mut extends_handled = false;
    // Tracks an alias for `t.prototype` introduced in Babel loose mode:
    //   `var proto = t.prototype;`
    let mut proto_alias: Option<Atom> = None;

    // We process in two passes:
    //  1. Locate constructor FnDecl / FnExpr
    //  2. Walk all other statements for method/property assignments
    //
    // Actually we do a single forward pass for simplicity.

    for stmt in stmts {
        // `return t;` or `return _createClass(t, ...)` or
        // `return t.method1 = fn, t.method2 = fn, ..., t;` — end of IIFE body
        if let Stmt::Return(ret_stmt) = stmt {
            if let Some(ret_expr) = &ret_stmt.arg {
                let stripped = strip_parens(ret_expr);
                match stripped {
                    // Plain `return t;`
                    Expr::Ident(id) if id.sym.as_ref() == inner_ctor_name => {
                        // Nothing to do
                    }
                    // `return _createClass(t, [{ key: "method", value: fn }], [{ ... }])`
                    Expr::Call(call) => {
                        if !try_parse_create_class(call, inner_ctor_name, &mut members) {
                            return None;
                        }
                    }
                    // `return t.method = fn, t.method2 = fn, ..., t;`
                    // Minified Babel loose: method assignments in comma/sequence expression
                    Expr::Seq(seq) => {
                        if !try_parse_seq_return(seq, inner_ctor_name, &proto_alias, &mut members) {
                            return None;
                        }
                    }
                    _ => return None,
                }
            }
            continue;
        }

        // `__extends(t, _super)` or `_inherits(t, _super)` or `customInherits(t, _super)`,
        // or inline IIFE: `((e, t) => { Object.create... })(t, _super)`
        if let Some(sp) = super_param {
            if try_parse_extends_call(stmt, inner_ctor_name, sp, inherits_helpers).is_some()
                || is_inline_inherits_iife(stmt, inner_ctor_name, sp)
            {
                extends_handled = true;
                continue;
            }
        }

        // `function t(...) { ... }` — the constructor
        if let Stmt::Decl(Decl::Fn(fn_decl)) = stmt {
            if fn_decl.ident.sym.as_ref() == inner_ctor_name {
                let ctor = build_constructor(&fn_decl.function, super_param)?;
                // Only add a constructor member if the body is non-empty
                if !is_empty_constructor(&fn_decl.function) {
                    members.push(ClassMember::Constructor(ctor));
                }
                continue;
            }
            return None;
        }

        // Expression statements
        if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
            // `t.prototype.method = function() { ... }`
            // `t.staticMethod = function() { ... }`
            // `t.prototype = Object.create(_super.prototype)` (inheritance setup — skip)
            // `Object.defineProperty(t.prototype, "prop", { get: fn, set: fn })`
            if try_parse_method_assignment(expr, inner_ctor_name, &proto_alias, &mut members) {
                continue;
            }

            // Babel loose mode: `Object.defineProperty(t.prototype, ...)`
            if try_parse_object_define_property(expr, inner_ctor_name, &proto_alias, &mut members) {
                continue;
            }

            // Skip `t.prototype = Object.create(...)` (prototype chain setup for inheritance)
            if is_prototype_object_create(expr, inner_ctor_name) {
                if super_param.is_some() {
                    extends_handled = true;
                }
                continue;
            }

            // Skip `t.prototype.constructor = t` (redundant constructor assignment)
            if is_prototype_constructor_assign(expr, inner_ctor_name) {
                continue;
            }

            // Skip inlined `_super && (Object.setPrototypeOf ? ...)` (static prototype chain)
            if let Some(sp) = super_param {
                if is_set_prototype_of_chain_expr(expr, sp) {
                    extends_handled = true;
                    continue;
                }
            }

            // `_createClass(t, [...], [...])` as a statement (Babel non-loose)
            if let Expr::Call(call) = expr.as_ref() {
                if try_parse_create_class(call, inner_ctor_name, &mut members) {
                    continue;
                }
            }

            return None;
        }

        // Babel loose mode: `var proto = t.prototype;`
        if let Stmt::Decl(Decl::Var(var_decl)) = stmt {
            if let Some(alias) = try_parse_proto_alias(var_decl, inner_ctor_name) {
                proto_alias = Some(alias);
                continue;
            }
            return None;
        }

        // Skip `if (typeof _super !== "function" && _super !== null) { throw ... }`
        if let Some(sp) = super_param {
            if is_super_typecheck_if_stmt(stmt, sp) {
                continue;
            }
        }

        return None;
    }

    // If the IIFE takes a _super param but we never saw __extends, reject
    if super_param.is_some() && !extends_handled {
        return None;
    }

    let _ = class_name; // used only for documentation purposes
    Some(members)
}

// ============================================================
// Statement parsers
// ============================================================

/// Detect `__extends(t, _super)`, `_inherits(t, _super)`, or a call to a detected inherits helper.
fn try_parse_extends_call(stmt: &Stmt, ctor_name: &str, super_param: &str, inherits_helpers: &HashSet<Atom>) -> Option<()> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let callee = strip_parens(callee);
    let Expr::Ident(fn_name) = callee else {
        return None;
    };
    // Accept known names or any detected inherits helper
    if fn_name.sym.as_ref() != "__extends"
        && fn_name.sym.as_ref() != "_inherits"
        && !inherits_helpers.contains(&fn_name.sym)
    {
        return None;
    }
    if call.args.len() != 2 {
        return None;
    }
    // First arg must be the inner constructor name
    let arg0 = strip_parens(&call.args[0].expr);
    if !matches!(arg0, Expr::Ident(id) if id.sym.as_ref() == ctor_name) {
        return None;
    }
    // Second arg must be the super param
    let arg1 = strip_parens(&call.args[1].expr);
    if !matches!(arg1, Expr::Ident(id) if id.sym.as_ref() == super_param) {
        return None;
    }
    Some(())
}

/// Detect `var proto = t.prototype` (Babel loose mode proto alias).
fn try_parse_proto_alias(var_decl: &VarDecl, ctor_name: &str) -> Option<Atom> {
    if var_decl.decls.len() != 1 {
        return None;
    }
    let d = &var_decl.decls[0];
    let Pat::Ident(BindingIdent { id: alias_id, .. }) = &d.name else {
        return None;
    };
    let init = d.init.as_ref()?;
    // Must be `t.prototype`
    if !is_prototype_member_expr(init, ctor_name) {
        return None;
    }
    Some(alias_id.sym.clone())
}

/// Try to parse `t.prototype.method = function...` or `t.staticProp = function...`
/// or `proto.method = function...` (when `proto_alias` is set).
///
/// Returns true if the expression was recognised and a class member was added.
fn try_parse_method_assignment(
    expr: &Expr,
    ctor_name: &str,
    proto_alias: &Option<Atom>,
    members: &mut Vec<ClassMember>,
) -> bool {
    let Expr::Assign(assign) = expr else {
        return false;
    };
    if assign.op != swc_core::ecma::ast::AssignOp::Assign {
        return false;
    }

    let swc_core::ecma::ast::AssignTarget::Simple(swc_core::ecma::ast::SimpleAssignTarget::Member(
        lhs_member,
    )) = &assign.left
    else {
        return false;
    };

    // Determine if this is a static or prototype method assignment
    //
    // Static:    `t.methodName = function() {}`
    // Prototype: `t.prototype.methodName = function() {}`
    // Loose:     `proto.methodName = function() {}` (proto_alias set)

    let (is_static, method_name) = if let Some(name) =
        extract_static_method_name(&lhs_member.obj, &lhs_member.prop, ctor_name)
    {
        (true, name)
    } else if let Some(name) =
        extract_proto_method_name(&lhs_member.obj, &lhs_member.prop, ctor_name, proto_alias)
    {
        (false, name)
    } else {
        return false;
    };

    // The RHS must be a function expression (named or anonymous)
    let rhs = strip_parens(&assign.right);
    let fn_expr = match rhs {
        Expr::Fn(f) => f,
        _ => return false,
    };

    let method = build_class_method(method_name, fn_expr, is_static, MethodKind::Method);
    members.push(ClassMember::Method(method));
    true
}

/// Try to parse `Object.defineProperty(t.prototype, "name", { get: fn, set: fn })`.
fn try_parse_object_define_property(
    expr: &Expr,
    ctor_name: &str,
    proto_alias: &Option<Atom>,
    members: &mut Vec<ClassMember>,
) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };

    // Callee must be `Object.defineProperty`
    if !is_object_define_property_callee(&call.callee) {
        return false;
    }

    if call.args.len() != 3 {
        return false;
    }

    // First arg: `t.prototype` or alias
    let target = strip_parens(&call.args[0].expr);
    let is_proto_target =
        is_prototype_member_expr(target, ctor_name) || is_proto_alias_expr(target, proto_alias);
    if !is_proto_target {
        return false;
    }

    // Second arg: property name (string literal)
    let prop_name_expr = strip_parens(&call.args[1].expr);
    let Expr::Lit(swc_core::ecma::ast::Lit::Str(s)) = prop_name_expr else {
        return false;
    };
    let sym: Atom = s.value.as_str().unwrap_or("").into();

    // Third arg: descriptor object `{ get: fn, set: fn, value: fn, ... }`
    let descriptor = strip_parens(&call.args[2].expr);
    let Expr::Object(obj) = descriptor else {
        return false;
    };

    for prop in &obj.props {
        let swc_core::ecma::ast::PropOrSpread::Prop(p) = prop else {
            continue;
        };
        let swc_core::ecma::ast::Prop::KeyValue(kv) = p.as_ref() else {
            continue;
        };
        let key_name = match &kv.key {
            PropName::Ident(iden) => iden.sym.clone(),
            PropName::Str(s) => s.value.as_str().unwrap_or("").into(),
            _ => continue,
        };
        let kind = match key_name.as_ref() {
            "get" => MethodKind::Getter,
            "set" => MethodKind::Setter,
            _ => continue,
        };
        let fn_expr = match strip_parens(&kv.value) {
            Expr::Fn(f) => f,
            _ => continue,
        };
        let method_key = PropName::Ident(IdentName::new(sym.clone(), DUMMY_SP));
        let method = build_class_method(method_key, fn_expr, false, kind);
        members.push(ClassMember::Method(method));
    }

    true
}

/// Parse a sequence expression in a return statement:
/// `return proto.method1 = fn, proto.method2 = fn, ..., ClassName;`
///
/// The last expression must be the constructor ident. Each preceding expression
/// must be a method/property assignment.
fn try_parse_seq_return(
    seq: &SeqExpr,
    ctor_name: &str,
    proto_alias: &Option<Atom>,
    members: &mut Vec<ClassMember>,
) -> bool {
    if seq.exprs.is_empty() {
        return false;
    }

    // Last expression must be the constructor ident
    let last = strip_parens(seq.exprs.last().unwrap());
    if !matches!(last, Expr::Ident(id) if id.sym.as_ref() == ctor_name) {
        return false;
    }

    // All preceding expressions must be method assignments
    for expr in &seq.exprs[..seq.exprs.len() - 1] {
        if !try_parse_method_assignment(strip_parens(expr), ctor_name, proto_alias, members) {
            return false;
        }
    }

    true
}

/// Parse `_createClass(t, instanceMethods, staticMethods)` where each methods array
/// contains `{ key: "name", value: function() {} }` objects.
fn try_parse_create_class(
    call: &CallExpr,
    ctor_name: &str,
    members: &mut Vec<ClassMember>,
) -> bool {
    // Callee must be `_createClass`
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !matches!(strip_parens(callee), Expr::Ident(id) if id.sym.as_ref() == "_createClass") {
        return false;
    }

    // Args: (ClassName, [instance methods], [static methods]?)
    if call.args.len() < 2 || call.args.len() > 3 {
        return false;
    }

    // First arg must be the constructor ident
    let arg0 = strip_parens(&call.args[0].expr);
    if !matches!(arg0, Expr::Ident(id) if id.sym.as_ref() == ctor_name) {
        return false;
    }

    // Instance methods
    if !parse_create_class_array(&call.args[1], false, members) {
        return false;
    }

    // Static methods (optional 3rd arg)
    if call.args.len() == 3 {
        if !parse_create_class_array(&call.args[2], true, members) {
            return false;
        }
    }

    true
}

fn parse_create_class_array(
    arg: &ExprOrSpread,
    is_static: bool,
    members: &mut Vec<ClassMember>,
) -> bool {
    let arr_expr = strip_parens(&arg.expr);
    // Allow `null` for the static array (Babel sometimes passes null)
    if matches!(arr_expr, Expr::Lit(swc_core::ecma::ast::Lit::Null(_))) {
        return true;
    }
    let Expr::Array(arr) = arr_expr else {
        return false;
    };

    for elem in &arr.elems {
        let Some(elem) = elem else {
            continue;
        };
        let Expr::Object(obj) = strip_parens(&elem.expr) else {
            return false;
        };

        let mut key_name: Option<Atom> = None;
        let mut value_fn: Option<&FnExpr> = None;
        let mut method_kind = MethodKind::Method;

        for prop in &obj.props {
            let swc_core::ecma::ast::PropOrSpread::Prop(p) = prop else {
                continue;
            };
            let swc_core::ecma::ast::Prop::KeyValue(kv) = p.as_ref() else {
                return false;
            };
            let k = match &kv.key {
                PropName::Ident(i) => i.sym.clone(),
                PropName::Str(s) => s.value.as_str().unwrap_or("").into(),
                _ => return false,
            };
            match k.as_ref() {
                "key" => {
                    let Expr::Lit(swc_core::ecma::ast::Lit::Str(s)) = strip_parens(&kv.value)
                    else {
                        return false;
                    };
                    key_name = Some(s.value.as_str().unwrap_or("").into());
                }
                "value" => {
                    let Expr::Fn(f) = strip_parens(&kv.value) else {
                        return false;
                    };
                    value_fn = Some(f);
                }
                "get" => {
                    let Expr::Fn(f) = strip_parens(&kv.value) else {
                        return false;
                    };
                    method_kind = MethodKind::Getter;
                    value_fn = Some(f);
                }
                "set" => {
                    let Expr::Fn(f) = strip_parens(&kv.value) else {
                        return false;
                    };
                    method_kind = MethodKind::Setter;
                    value_fn = Some(f);
                }
                // `writable`, `enumerable`, `configurable` — skip
                _ => {}
            }
        }

        let (Some(name_sym), Some(fn_expr)) = (key_name, value_fn) else {
            return false;
        };
        let method_key = PropName::Ident(IdentName::new(name_sym, DUMMY_SP));
        let method = build_class_method(method_key, fn_expr, is_static, method_kind);
        members.push(ClassMember::Method(method));
    }

    true
}

// ============================================================
// Detection helpers
// ============================================================

/// Find the name of the inner constructor function (`t` in the IIFE body).
/// The first `function <name>(...) { ... }` declaration in the body is the constructor.
fn find_inner_constructor_name(stmts: &[Stmt]) -> Option<&str> {
    for stmt in stmts {
        if let Stmt::Decl(Decl::Fn(fn_decl)) = stmt {
            return Some(fn_decl.ident.sym.as_ref());
        }
    }
    None
}

/// Return true if `expr` is `t.prototype` where `t` is `ctor_name`.
fn is_prototype_member_expr(expr: &Expr, ctor_name: &str) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    let Expr::Ident(obj_id) = obj.as_ref() else {
        return false;
    };
    if obj_id.sym.as_ref() != ctor_name {
        return false;
    }
    matches!(prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype")
}

/// Return true if `expr` matches the proto alias identifier.
fn is_proto_alias_expr(expr: &Expr, proto_alias: &Option<Atom>) -> bool {
    let Some(alias) = proto_alias else {
        return false;
    };
    matches!(expr, Expr::Ident(id) if &id.sym == alias)
}

/// Check if `expr` is `t.prototype = Object.create(...)`.
fn is_prototype_object_create(expr: &Expr, ctor_name: &str) -> bool {
    let Expr::Assign(assign) = expr else {
        return false;
    };
    if assign.op != swc_core::ecma::ast::AssignOp::Assign {
        return false;
    }
    let swc_core::ecma::ast::AssignTarget::Simple(swc_core::ecma::ast::SimpleAssignTarget::Member(
        lhs,
    )) = &assign.left
    else {
        return false;
    };
    // LHS: `t.prototype`
    if !is_prototype_member_expr(
        &Expr::Member(MemberExpr {
            span: DUMMY_SP,
            obj: lhs.obj.clone(),
            prop: lhs.prop.clone(),
        }),
        ctor_name,
    ) {
        return false;
    }
    // RHS: `Object.create(...)`
    let rhs = strip_parens(&assign.right);
    let Expr::Call(call) = rhs else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    is_object_create_callee(callee)
}

/// Check if `expr` is `t.prototype.constructor = t`.
fn is_prototype_constructor_assign(expr: &Expr, ctor_name: &str) -> bool {
    let Expr::Assign(assign) = expr else {
        return false;
    };
    if assign.op != swc_core::ecma::ast::AssignOp::Assign {
        return false;
    }
    let swc_core::ecma::ast::AssignTarget::Simple(swc_core::ecma::ast::SimpleAssignTarget::Member(
        lhs,
    )) = &assign.left
    else {
        return false;
    };
    // LHS must be `t.prototype.constructor`
    let Expr::Member(obj_member) = lhs.obj.as_ref() else {
        return false;
    };
    if !is_prototype_member_expr(
        &Expr::Member(MemberExpr {
            span: DUMMY_SP,
            obj: obj_member.obj.clone(),
            prop: obj_member.prop.clone(),
        }),
        ctor_name,
    ) {
        return false;
    }
    if !matches!(&lhs.prop, MemberProp::Ident(n) if n.sym.as_ref() == "constructor") {
        return false;
    }
    // RHS must be `t`
    matches!(strip_parens(&assign.right), Expr::Ident(id) if id.sym.as_ref() == ctor_name)
}

/// Extract the property name for a **static** assignment `t.prop = ...`.
/// Returns `Some(PropName)` if `obj` is the constructor ident and `prop` is a static method name
/// (not `prototype`).
fn extract_static_method_name(obj: &Expr, prop: &MemberProp, ctor_name: &str) -> Option<PropName> {
    let Expr::Ident(obj_id) = obj else {
        return None;
    };
    if obj_id.sym.as_ref() != ctor_name {
        return None;
    }
    match prop {
        MemberProp::Ident(name) => {
            // Skip `t.prototype`
            if name.sym.as_ref() == "prototype" {
                return None;
            }
            Some(PropName::Ident(IdentName::new(name.sym.clone(), DUMMY_SP)))
        }
        MemberProp::Computed(c) => {
            if let Expr::Lit(swc_core::ecma::ast::Lit::Str(s)) = strip_parens(&c.expr) {
                Some(PropName::Str(swc_core::ecma::ast::Str {
                    span: DUMMY_SP,
                    value: s.value.clone(),
                    raw: None,
                }))
            } else {
                Some(PropName::Computed(ComputedPropName {
                    span: DUMMY_SP,
                    expr: c.expr.clone(),
                }))
            }
        }
        _ => None,
    }
}

/// Extract the property name for a **prototype** assignment.
///
/// Handles:
///   `t.prototype.method` where `obj` is `t.prototype`
///   `proto.method` where `obj` is the proto alias
fn extract_proto_method_name(
    obj: &Expr,
    prop: &MemberProp,
    ctor_name: &str,
    proto_alias: &Option<Atom>,
) -> Option<PropName> {
    let obj_is_proto =
        is_prototype_member_expr(obj, ctor_name) || is_proto_alias_expr(obj, proto_alias);
    if !obj_is_proto {
        return None;
    }
    // Skip the constructor property
    if matches!(prop, MemberProp::Ident(n) if n.sym.as_ref() == "constructor") {
        return None;
    }
    match prop {
        MemberProp::Ident(name) => {
            Some(PropName::Ident(IdentName::new(name.sym.clone(), DUMMY_SP)))
        }
        MemberProp::Computed(c) => {
            if let Expr::Lit(swc_core::ecma::ast::Lit::Str(s)) = strip_parens(&c.expr) {
                Some(PropName::Str(swc_core::ecma::ast::Str {
                    span: DUMMY_SP,
                    value: s.value.clone(),
                    raw: None,
                }))
            } else {
                Some(PropName::Computed(ComputedPropName {
                    span: DUMMY_SP,
                    expr: c.expr.clone(),
                }))
            }
        }
        _ => None,
    }
}

fn is_object_define_property_callee(callee: &Callee) -> bool {
    let Callee::Expr(expr) = callee else {
        return false;
    };
    let Expr::Member(m) = strip_parens(expr) else {
        return false;
    };
    let Expr::Ident(obj_id) = m.obj.as_ref() else {
        return false;
    };
    if obj_id.sym.as_ref() != "Object" {
        return false;
    }
    matches!(&m.prop, MemberProp::Ident(n) if n.sym.as_ref() == "defineProperty")
}

fn is_object_create_callee(expr: &Expr) -> bool {
    let Expr::Member(m) = strip_parens(expr) else {
        return false;
    };
    let Expr::Ident(obj_id) = m.obj.as_ref() else {
        return false;
    };
    if obj_id.sym.as_ref() != "Object" {
        return false;
    }
    matches!(&m.prop, MemberProp::Ident(n) if n.sym.as_ref() == "create")
}

/// Check `_super && (Object.setPrototypeOf ? Object.setPrototypeOf(t, _super) : t.__proto__ = _super)`.
/// This is the inlined static prototype chain setup emitted by webpack4 instead of `_inherits`.
fn is_set_prototype_of_chain_expr(expr: &Expr, super_param: &str) -> bool {
    let Expr::Bin(bin) = expr else {
        return false;
    };
    if bin.op != swc_core::ecma::ast::BinaryOp::LogicalAnd {
        return false;
    }
    // Left must be the super param ident
    if !matches!(strip_parens(&bin.left), Expr::Ident(id) if id.sym.as_ref() == super_param) {
        return false;
    }
    // Right must be a conditional whose test is `Object.setPrototypeOf`
    let Expr::Cond(cond) = strip_parens(&bin.right) else {
        return false;
    };
    let Expr::Member(m) = strip_parens(&cond.test) else {
        return false;
    };
    let Expr::Ident(obj_id) = m.obj.as_ref() else {
        return false;
    };
    obj_id.sym.as_ref() == "Object"
        && matches!(&m.prop, MemberProp::Ident(n) if n.sym.as_ref() == "setPrototypeOf")
}

/// Check `if (typeof _super !== "function" && _super !== null) { throw ... }`.
/// Detect inline IIFE `_inherits` pattern:
/// ```js
/// ((e, t) => {
///     if (typeof t != "function" && t !== null) throw TypeError(...)
///     e.prototype = Object.create(t && t.prototype, { constructor: ... })
///     t && (Object.setPrototypeOf ? ... : ...)
/// })(ctor, super)
/// ```
fn is_inline_inherits_iife(stmt: &Stmt, ctor_name: &str, super_param: &str) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };

    // Must have 2 args: (ctor, super)
    if call.args.len() != 2 {
        return false;
    }
    let arg0 = strip_parens(&call.args[0].expr);
    let arg1 = strip_parens(&call.args[1].expr);
    if !matches!(arg0, Expr::Ident(id) if id.sym.as_ref() == ctor_name) {
        return false;
    }
    if !matches!(arg1, Expr::Ident(id) if id.sym.as_ref() == super_param) {
        return false;
    }

    // Callee must be an arrow or function with 2 params
    let inner = strip_parens(callee);
    let body_stmts: &[Stmt] = match inner {
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 2 {
                return false;
            }
            match &*arrow.body {
                BlockStmtOrExpr::BlockStmt(block) => &block.stmts,
                _ => return false,
            }
        }
        Expr::Fn(fn_expr) => {
            if fn_expr.function.params.len() != 2 {
                return false;
            }
            match &fn_expr.function.body {
                Some(block) => &block.stmts,
                None => return false,
            }
        }
        _ => return false,
    };

    // Body should contain Object.create (prototype chain setup)
    body_stmts.iter().any(|s| stmt_has_object_create(s))
}

/// Check if a statement contains `Object.create(...)` call.
fn stmt_has_object_create(stmt: &Stmt) -> bool {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct Finder {
        found: bool,
    }
    impl Visit for Finder {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Member(member) = callee.as_ref() {
                    if let Expr::Ident(obj) = member.obj.as_ref() {
                        if obj.sym.as_ref() == "Object" {
                            if let MemberProp::Ident(prop) = &member.prop {
                                if prop.sym.as_ref() == "create" {
                                    self.found = true;
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder { found: false };
    stmt.visit_with(&mut finder);
    finder.found
}

fn is_super_typecheck_if_stmt(stmt: &Stmt, super_param: &str) -> bool {
    let Stmt::If(if_stmt) = stmt else {
        return false;
    };
    let Expr::Bin(bin) = strip_parens(&if_stmt.test) else {
        return false;
    };
    if bin.op != swc_core::ecma::ast::BinaryOp::LogicalAnd {
        return false;
    }
    is_typeof_not_function(strip_parens(&bin.left), super_param)
        && is_not_null_check(strip_parens(&bin.right), super_param)
}

/// Return true if `expr` is `typeof name !== "function"`.
fn is_typeof_not_function(expr: &Expr, name: &str) -> bool {
    let Expr::Bin(bin) = expr else {
        return false;
    };
    if bin.op != swc_core::ecma::ast::BinaryOp::NotEqEq {
        return false;
    }
    let Expr::Unary(u) = strip_parens(&bin.left) else {
        return false;
    };
    if u.op != swc_core::ecma::ast::UnaryOp::TypeOf {
        return false;
    }
    if !matches!(strip_parens(&u.arg), Expr::Ident(id) if id.sym.as_ref() == name) {
        return false;
    }
    matches!(
        strip_parens(&bin.right),
        Expr::Lit(swc_core::ecma::ast::Lit::Str(s)) if s.value.as_str() == Some("function")
    )
}

/// Return true if `expr` is `name !== null`.
fn is_not_null_check(expr: &Expr, name: &str) -> bool {
    let Expr::Bin(bin) = expr else {
        return false;
    };
    if bin.op != swc_core::ecma::ast::BinaryOp::NotEqEq {
        return false;
    }
    if !matches!(strip_parens(&bin.left), Expr::Ident(id) if id.sym.as_ref() == name) {
        return false;
    }
    matches!(
        strip_parens(&bin.right),
        Expr::Lit(swc_core::ecma::ast::Lit::Null(_))
    )
}

// ============================================================
// Builder helpers
// ============================================================

fn build_constructor(function: &Function, super_param: Option<&str>) -> Option<Constructor> {
    let mut body = function.body.clone()?;
    let params: Vec<ParamOrTsParamProp> = function
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

    if let Some(sp) = super_param {
        // Unwrap inline _possibleConstructorReturn IIFEs before super rewriting
        unwrap_inline_pcr_iife(&mut body);
        // Rewrite `superParam.call(this, ...)` → `super(...)` in the constructor body
        body.visit_mut_with(&mut SuperCallRewriter {
            super_param_name: sp,
        });
        // Simplify `super(...) || this` → `super(...)` — the `|| this` is dead code
        // per ES6 spec (super() always returns `this` in derived constructors)
        body.visit_mut_with(&mut SuperOrThisSimplifier);
        // Split `(alias = super(...)).prop = value` into `alias = super(...); alias.prop = value`
        // so that cleanup_super_aliases can detect the alias pattern.
        split_assign_prop_chains(&mut body);
        // Clean up super() aliases: in `n = r = super()`, both n and r are `this`.
        // Replace references with `this`, remove var decls and trailing `return alias`.
        cleanup_super_aliases(&mut body);
        // Strip `return super(...)` → `super(...)` (constructors return implicitly)
        strip_return_super(&mut body);
    }

    Some(Constructor {
        span: DUMMY_SP,
        ctxt: Default::default(),
        key: PropName::Ident(IdentName::new("constructor".into(), DUMMY_SP)),
        params,
        body: Some(body),
        accessibility: None,
        is_optional: false,
    })
}

/// Detect and unwrap inline `_possibleConstructorReturn` IIFEs in constructor bodies.
///
/// Pattern:
/// ```js
/// return (function(e, t) {
///     if (!e) throw new ReferenceError("...");
///     return !t || ... ? e : t;
/// })(this, superCall);
/// ```
/// or the arrow variant: `((e, t) => { ... })(this, superCall)`
///
/// Replaced with: `return superCall`
fn unwrap_inline_pcr_iife(body: &mut BlockStmt) {
    for stmt in body.stmts.iter_mut() {
        // Handle `return pcrIIFE(this, expr)` → `return expr`
        if let Stmt::Return(ret) = stmt {
            if let Some(arg) = &mut ret.arg {
                if let Some(unwrapped) = try_unwrap_pcr_expr(arg) {
                    *arg = Box::new(unwrapped);
                }
            }
        }
        // Handle `pcrIIFE(this, expr)` as expression statement → `expr`
        if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
            if let Some(unwrapped) = try_unwrap_pcr_expr(expr) {
                *expr = Box::new(unwrapped);
            }
        }
    }
}

/// Try to unwrap an expression if it's an inline PCR IIFE call.
/// Also handles comma expressions like `classCallCheck(this, t), pcrIIFE(this, expr)`.
fn try_unwrap_pcr_expr(expr: &Expr) -> Option<Expr> {
    // Handle comma/sequence expression: `a, b` — check the last element
    if let Expr::Seq(seq) = expr {
        if let Some(last) = seq.exprs.last() {
            if let Some(unwrapped) = try_unwrap_pcr_call(last) {
                return Some(unwrapped);
            }
        }
        return None;
    }
    try_unwrap_pcr_call(expr)
}

/// Check if expr is an IIFE call matching the _possibleConstructorReturn body shape,
/// and return the second argument if so.
fn try_unwrap_pcr_call(expr: &Expr) -> Option<Expr> {
    let Expr::Call(call) = expr else { return None };
    let Callee::Expr(callee) = &call.callee else { return None };
    let inner = strip_parens(callee);

    // Must have exactly 2 args: (this, superCall)
    if call.args.len() != 2 {
        return None;
    }
    // First arg must be `this`
    if !matches!(strip_parens(&call.args[0].expr), Expr::This(..)) {
        return None;
    }

    // Extract params and body from the callee (function or arrow)
    let (params, body_stmts): (Vec<&Pat>, &[Stmt]) = match inner {
        Expr::Fn(fn_expr) => {
            let body = fn_expr.function.body.as_ref()?;
            let pats: Vec<&Pat> = fn_expr.function.params.iter().map(|p| &p.pat).collect();
            (pats, &body.stmts)
        }
        Expr::Arrow(arrow) => {
            let BlockStmtOrExpr::BlockStmt(block) = &*arrow.body else {
                return None;
            };
            let pats: Vec<&Pat> = arrow.params.iter().collect();
            (pats, &block.stmts)
        }
        _ => return None,
    };

    // Must have 2 params
    if params.len() != 2 {
        return None;
    }
    let Pat::Ident(param1) = params[0] else { return None };

    // First statement must be `if (!param1) throw new ReferenceError(...)`
    if body_stmts.is_empty() {
        return None;
    }
    if !is_pcr_guard_stmt(&body_stmts[0], &param1.id.sym) {
        return None;
    }

    // Matches! Return the second argument (the super call)
    Some(*call.args[1].expr.clone())
}

/// Check if a statement matches `if (!param) throw new ReferenceError(...)`.
/// Requires the throw to construct a `ReferenceError` specifically to avoid
/// false-positives on other guard-shaped inline functions.
fn is_pcr_guard_stmt(stmt: &Stmt, param_name: &str) -> bool {
    let Stmt::If(if_stmt) = stmt else { return false };
    let Expr::Unary(unary) = if_stmt.test.as_ref() else { return false };
    if unary.op != swc_core::ecma::ast::UnaryOp::Bang {
        return false;
    }
    if !matches!(strip_parens(&unary.arg), Expr::Ident(id) if id.sym.as_ref() == param_name) {
        return false;
    }
    // Consequent should throw ReferenceError (block or direct)
    let throw_expr = match if_stmt.cons.as_ref() {
        Stmt::Throw(t) => Some(&t.arg),
        Stmt::Block(block) if block.stmts.len() == 1 => {
            if let Stmt::Throw(t) = &block.stmts[0] {
                Some(&t.arg)
            } else {
                None
            }
        }
        _ => None,
    };
    let Some(throw_arg) = throw_expr else { return false };
    is_new_reference_error(throw_arg)
}

/// Check if an expression is `new ReferenceError(...)`.
fn is_new_reference_error(expr: &Expr) -> bool {
    let Expr::New(new_expr) = strip_parens(expr) else { return false };
    matches!(strip_parens(&new_expr.callee), Expr::Ident(id) if id.sym.as_ref() == "ReferenceError")
}

/// Strip `return super(...)` at the end of a constructor body → `super(...)` as expr stmt.
/// In derived constructors, `return super()` is unnecessary; super() implicitly returns this.
fn strip_return_super(body: &mut BlockStmt) {
    if let Some(last) = body.stmts.last() {
        if let Stmt::Return(ret) = last {
            if let Some(arg) = &ret.arg {
                if is_super_call(arg) {
                    let super_call = arg.clone();
                    let len = body.stmts.len();
                    body.stmts[len - 1] = Stmt::Expr(ExprStmt {
                        span: DUMMY_SP,
                        expr: super_call,
                    });
                }
            }
        }
    }
}

/// Split `(alias = super(...)).prop = value` into two statements:
///   `alias = super(...);`
///   `alias.prop = value;`
///
/// Babel minifiers produce `(o = super()).x = 1, o.y = 2, ...` where the assignment
/// and property access are fused. Splitting allows `cleanup_super_aliases` to detect
/// the alias and replace `o.x` with `this.x`.
fn split_assign_prop_chains(body: &mut BlockStmt) {
    let mut new_stmts = Vec::with_capacity(body.stmts.len());
    for stmt in std::mem::take(&mut body.stmts) {
        if let Stmt::Expr(ExprStmt { expr, span }) = &stmt {
            if let Some((assign_stmt, prop_stmt)) = try_split_assign_prop(expr, *span) {
                new_stmts.push(assign_stmt);
                new_stmts.push(prop_stmt);
                continue;
            }
        }
        new_stmts.push(stmt);
    }
    body.stmts = new_stmts;
}

/// Try to split `(alias = super(...)).prop = value` into two statements.
fn try_split_assign_prop(expr: &Expr, span: swc_core::common::Span) -> Option<(Stmt, Stmt)> {
    let Expr::Assign(outer) = expr else { return None };
    if outer.op != AssignOp::Assign {
        return None;
    }

    // LHS must be a member expression whose object is a paren-wrapped assignment
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &outer.left else {
        return None;
    };

    let obj = strip_parens(&member.obj);
    let Expr::Assign(inner) = obj else { return None };
    if inner.op != AssignOp::Assign {
        return None;
    }

    // The inner RHS must be a super call
    if !is_super_call(&inner.right) {
        return None;
    }

    // Extract the alias ident
    let AssignTarget::Simple(SimpleAssignTarget::Ident(alias)) = &inner.left else {
        return None;
    };

    // Statement 1: alias = super(...)
    let assign_stmt = Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: inner.left.clone(),
            right: inner.right.clone(),
        })),
    });

    // Statement 2: alias.prop = value
    let prop_stmt = Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: outer.op,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Ident(Ident::new(
                    alias.sym.clone(),
                    DUMMY_SP,
                    alias.ctxt,
                ))),
                prop: member.prop.clone(),
            })),
            right: outer.right.clone(),
        })),
    });

    Some((assign_stmt, prop_stmt))
}

/// Simplifies `super(...) || this` → `super(...)` in constructor bodies.
/// In ES6, super() in a derived constructor always returns `this`, so the
/// `|| this` fallback is dead code. Removing it allows cleanup_super_aliases
/// to recognize the pattern.
struct SuperOrThisSimplifier;

impl VisitMut for SuperOrThisSimplifier {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Bin(bin) = expr else { return };
        if bin.op != swc_core::ecma::ast::BinaryOp::LogicalOr {
            return;
        }
        if !is_super_call(&bin.left) {
            return;
        }
        if !matches!(*bin.right, Expr::This(..)) {
            return;
        }
        // Replace `super(...) || this` with just `super(...)`
        *expr = *bin.left.clone();
    }

    // Don't descend into nested functions/arrows
    fn visit_mut_function(&mut self, _: &mut Function) {}
    fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}
}

/// Rewrites `superParam.call(this, args...)` → `super(args...)`
/// and `superParam.apply(this, arguments)` → `super(...arguments)` in constructor bodies.
struct SuperCallRewriter<'a> {
    super_param_name: &'a str,
}

impl VisitMut for SuperCallRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else { return };
        let Expr::Member(member) = callee.as_ref() else { return };

        // Check: superParam.call or superParam.apply
        let Expr::Ident(obj_id) = member.obj.as_ref() else { return };
        if obj_id.sym.as_ref() != self.super_param_name {
            return;
        }
        let MemberProp::Ident(prop) = &member.prop else { return };

        match prop.sym.as_ref() {
            "call" => {
                // Must have at least 1 arg (the `this` arg)
                if call.args.is_empty() {
                    return;
                }
                // First arg should be `this` — skip it, rest become super() args
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
                // e.apply(this, arguments) → super(...arguments)
                // Only handle when second arg is `arguments` — other values like
                // null/undefined have different semantics with .apply vs spread.
                if call.args.len() != 2 {
                    return;
                }
                if !matches!(call.args[0].expr.as_ref(), Expr::This(..)) {
                    return;
                }
                // Second arg must be `arguments` or an array literal for safe conversion
                let second = strip_parens(&call.args[1].expr);
                let is_safe = matches!(second, Expr::Ident(id) if id.sym.as_ref() == "arguments")
                    || matches!(second, Expr::Array(_));
                if !is_safe {
                    return;
                }
                // Second arg becomes a spread argument to super()
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
}

/// In a derived constructor, `super()` returns `this`. Clean up:
/// - `var r = super(...)` → `super(...)`; mark `r` as this-alias
/// - `n = r = super(...)` → `super(...)`; mark both as this-aliases
/// - Replace all references to aliases with `this`
/// - Remove `var` declarations for aliases
/// - Remove trailing `return alias`
fn cleanup_super_aliases(body: &mut BlockStmt) {
    use std::collections::HashSet;

    let mut aliases: HashSet<Atom> = HashSet::new();

    // Pass 1: Find super() call statements and collect aliases
    for stmt in body.stmts.iter_mut() {
        // Pattern: `var r = super(...)` as a var decl
        if let Stmt::Decl(Decl::Var(var)) = stmt {
            for decl in &var.decls {
                if let (Pat::Ident(bi), Some(init)) = (&decl.name, &decl.init) {
                    if is_super_call(init) {
                        aliases.insert(bi.id.sym.clone());
                    }
                }
            }
        }

        // Pattern: `n = r = super(...)` or `r = super(...)` as expr stmt
        if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
            collect_assign_chain_aliases(expr, &mut aliases);
        }
    }

    if aliases.is_empty() {
        return;
    }

    // Pass 2: Rewrite alias references → `this`
    body.visit_mut_with(&mut AliasToThisRewriter { aliases: &aliases });

    // Pass 3: Rewrite statements — remove alias decls, simplify assign chains,
    // replace alias references with `this`, remove trailing `return alias`.
    let mut new_stmts = Vec::with_capacity(body.stmts.len());
    for stmt in std::mem::take(&mut body.stmts) {
        match stmt {
            // `var n;` (bare alias) → drop
            // `var r = super(...)` → keep `super(...)` as expr stmt
            Stmt::Decl(Decl::Var(mut var)) => {
                let mut keep_decls = Vec::new();
                for d in std::mem::take(&mut var.decls) {
                    let Pat::Ident(ref bi) = d.name else {
                        keep_decls.push(d);
                        continue;
                    };
                    if !aliases.contains(&bi.id.sym) {
                        keep_decls.push(d);
                        continue;
                    }
                    // Alias decl: extract super() call as statement if present
                    if let Some(init) = &d.init {
                        if is_super_call(init) {
                            new_stmts.push(Stmt::Expr(ExprStmt {
                                span: DUMMY_SP,
                                expr: init.clone(),
                            }));
                        }
                    }
                    // Drop the var declarator
                }
                if !keep_decls.is_empty() {
                    var.decls = keep_decls;
                    new_stmts.push(Stmt::Decl(Decl::Var(var)));
                }
            }
            // `n = r = super(...)` → `super(...)`
            Stmt::Expr(ExprStmt { ref expr, span }) => {
                if let Some(super_call) = extract_super_from_assign_chain(expr, &aliases) {
                    new_stmts.push(Stmt::Expr(ExprStmt {
                        expr: Box::new(super_call),
                        span,
                    }));
                } else {
                    new_stmts.push(stmt);
                }
            }
            // `return alias` → drop (constructor implicitly returns this)
            Stmt::Return(ref ret) => {
                let should_drop = ret.arg.as_ref().is_some_and(|arg| {
                    matches!(arg.as_ref(), Expr::Ident(id) if aliases.contains(&id.sym))
                        || matches!(arg.as_ref(), Expr::This(..))
                });
                if !should_drop {
                    new_stmts.push(stmt);
                }
            }
            other => new_stmts.push(other),
        }
    }
    body.stmts = new_stmts;
}

fn is_super_call(expr: &Expr) -> bool {
    matches!(expr, Expr::Call(call) if matches!(call.callee, Callee::Super(..)))
}

/// Walk an assignment chain like `n = r = super()` and collect all LHS idents as aliases.
fn collect_assign_chain_aliases(expr: &Expr, aliases: &mut std::collections::HashSet<Atom>) {
    let Expr::Assign(assign) = expr else { return };
    if assign.op != AssignOp::Assign {
        return;
    }

    // Check if the RHS is super() or another assignment chain ending in super()
    let rhs_is_super = is_super_call(&assign.right)
        || matches!(assign.right.as_ref(), Expr::Assign(_) if {
            let mut inner_aliases = std::collections::HashSet::new();
            collect_assign_chain_aliases(&assign.right, &mut inner_aliases);
            !inner_aliases.is_empty()
        });

    if rhs_is_super {
        if let AssignTarget::Simple(SimpleAssignTarget::Ident(id)) = &assign.left {
            aliases.insert(id.sym.clone());
        }
        // Recurse into RHS for chained assigns
        collect_assign_chain_aliases(&assign.right, aliases);
    }
}

/// Extract the super() call from an assignment chain like `n = r = super(...)`.
/// Returns Some(super_call_expr) if all LHS idents are known aliases, None otherwise.
fn extract_super_from_assign_chain(
    expr: &Expr,
    aliases: &std::collections::HashSet<Atom>,
) -> Option<Expr> {
    let Expr::Assign(assign) = expr else { return None };
    if assign.op != AssignOp::Assign {
        return None;
    }

    // LHS must be an alias
    let is_alias_lhs = matches!(
        &assign.left,
        AssignTarget::Simple(SimpleAssignTarget::Ident(id)) if aliases.contains(&id.sym)
    );
    if !is_alias_lhs {
        return None;
    }

    // RHS is super() → return super()
    if is_super_call(&assign.right) {
        return Some(*assign.right.clone());
    }

    // RHS is another alias = super() chain → recurse
    extract_super_from_assign_chain(&assign.right, aliases)
}

struct AliasToThisRewriter<'a> {
    aliases: &'a std::collections::HashSet<Atom>,
}

impl VisitMut for AliasToThisRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Ident(id) = expr {
            if self.aliases.contains(&id.sym) {
                *expr = Expr::This(swc_core::ecma::ast::ThisExpr { span: DUMMY_SP });
            }
        }
    }

    // Don't descend into nested functions/arrows
    fn visit_mut_function(&mut self, _: &mut Function) {}
    fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}
}

fn is_empty_constructor(function: &Function) -> bool {
    match &function.body {
        None => true,
        Some(BlockStmt { stmts, .. }) => stmts.is_empty(),
    }
}

fn build_class_method(
    key: PropName,
    fn_expr: &FnExpr,
    is_static: bool,
    kind: MethodKind,
) -> ClassMethod {
    ClassMethod {
        span: DUMMY_SP,
        key,
        function: fn_expr.function.clone(),
        kind,
        is_static,
        accessibility: None,
        is_abstract: false,
        is_optional: false,
        is_override: false,
    }
}
