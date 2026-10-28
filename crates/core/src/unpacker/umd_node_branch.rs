//! The factory call a plain UMD wrapper makes when Node loads the file as
//! CommonJS.
//!
//! Unpack output follows Node's module rules, so the wrapper branch that
//! matters is the one Node takes. This module walks the wrapper body with the
//! values Node gives the globals a UMD wrapper tests (`typeof module`,
//! `typeof define`, ...) and accepts the wrapper only when that walk reaches
//! exactly one factory call whose arguments are each `exports`, `module`,
//! `require`, or `require("<literal>")`, called for its effects or assigned to
//! `module.exports`. Generated UMD (rollup, Babel's
//! `@babel/plugin-transform-modules-umd`, webpack's `umd` library target)
//! takes that form. Anything the walk cannot decide (a test on a wrapper
//! parameter, a conditional factory call, an argument built from a global)
//! fails closed and the caller keeps the wrapper.
//!
//! The input is matched before the resolver runs. That is sound here because
//! the caller accepts only a file whose sole statement is the wrapper call:
//! a free name in the wrapper is a global unless the wrapper itself declares
//! it, and those names (its parameters and hoisted declarations) are
//! treated as unknown.

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, AssignOp, AssignTarget, BinaryOp, CallExpr, Callee, Decl, Expr,
    FnDecl, FnExpr, Function, FunctionBody, Lit, MemberExpr, MemberProp, Pat, SimpleAssignTarget,
    Stmt, UnaryOp, VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::collections::{HashMap, HashSet};
use crate::utils::paren::strip_parens;

/// One argument of the factory call on Node's branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UmdArg {
    Exports,
    Module,
    Require,
    RequireCall(Atom),
}

/// What Node's branch does with the factory's return value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UmdResult {
    /// `factory(...)`: the return value is discarded.
    Discarded,
    /// `module.exports = factory(...)`.
    ModuleExports,
}

pub(super) struct UmdNodeCall<'a> {
    pub factory: &'a Expr,
    pub args: Vec<UmdArg>,
    pub result: UmdResult,
}

/// The factory call Node makes for a top-level UMD wrapper statement, or
/// `None` when the statement is not a UMD wrapper call or Node's branch is
/// not a plain factory call.
pub(super) fn umd_node_call(expr: &Expr) -> Option<UmdNodeCall<'_>> {
    let call = top_level_call(expr)?;
    let (wrapper_params, wrapper_body) = wrapper_parts(&call.callee)?;
    let factory_sym = wrapper_params.get(1)?.clone();
    if !super::wrappers::body_looks_like_umd_wrapper(wrapper_body, &factory_sym) {
        return None;
    }

    let factory_arg = call.args.get(1)?;
    if factory_arg.spread.is_some() {
        return None;
    }
    let factory = strip_parens(&factory_arg.expr);
    if !matches!(factory, Expr::Fn(_) | Expr::Arrow(_)) {
        return None;
    }
    // The other wrapper arguments (the `root` / `global` value) are evaluated
    // before the wrapper runs, and the lift drops them. That is sound only
    // when evaluating them has no effect. A read of a browser global such as
    // `window` throws under Node; dropping it lets the factory run where the
    // original threw, and changes nothing the original did.
    for (index, arg) in call.args.iter().enumerate() {
        if index != 1 && (arg.spread.is_some() || !is_effect_free(&arg.expr)) {
            return None;
        }
    }

    let declared = HoistedNames::of_stmts(&wrapper_body.stmts);
    // A wrapper that declares the factory's name again may call something
    // else under that name.
    if declared.names.contains(&factory_sym) {
        return None;
    }
    let mut shadowed: HashSet<Atom> = wrapper_params.iter().cloned().collect();
    shadowed.extend(declared.names);
    let mut walk = BranchWalk {
        env: Env {
            shadowed,
            locals: HashMap::default(),
        },
        factory_sym,
        found: None,
    };
    for stmt in &wrapper_body.stmts {
        walk.stmt(stmt)?;
    }
    let (factory_call, result) = walk.found?;
    let args = factory_call
        .args
        .iter()
        .map(|arg| {
            if arg.spread.is_some() {
                return None;
            }
            walk.env.factory_arg(&arg.expr)
        })
        .collect::<Option<Vec<_>>>()?;
    Some(UmdNodeCall {
        factory,
        args,
        result,
    })
}

fn top_level_call(expr: &Expr) -> Option<&CallExpr> {
    match strip_parens(expr) {
        Expr::Call(call) => Some(call),
        Expr::Unary(unary) if matches!(unary.op, UnaryOp::Bang) => match strip_parens(&unary.arg) {
            Expr::Call(call) => Some(call),
            _ => None,
        },
        _ => None,
    }
}

/// A synchronous wrapper function whose parameters are all plain names.
fn wrapper_parts(callee: &Callee) -> Option<(Vec<Atom>, &FunctionBody)> {
    let Callee::Expr(callee) = callee else {
        return None;
    };
    match strip_parens(callee) {
        Expr::Fn(FnExpr { function, .. }) if !function.is_async && !function.is_generator => {
            let params = function
                .params
                .iter()
                .map(|param| ident_pat_sym(&param.pat))
                .collect::<Option<Vec<_>>>()?;
            Some((params, function.body.as_ref()?))
        }
        Expr::Arrow(ArrowExpr {
            params,
            body,
            is_async: false,
            ..
        }) => {
            let ArrowFunctionBody::FunctionBody(body) = &**body else {
                return None;
            };
            let params = params
                .iter()
                .map(ident_pat_sym)
                .collect::<Option<Vec<_>>>()?;
            Some((params, body))
        }
        _ => None,
    }
}

fn ident_pat_sym(pat: &Pat) -> Option<Atom> {
    match pat {
        Pat::Ident(binding) => Some(binding.sym.clone()),
        _ => None,
    }
}

/// An abstract value under Node's CommonJS module scope.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Str(Atom),
    Bool(bool),
    Undefined,
    Object,
    Function,
}

impl Value {
    fn type_of(&self) -> &'static str {
        match self {
            Value::Str(_) => "string",
            Value::Bool(_) => "boolean",
            Value::Undefined => "undefined",
            Value::Object => "object",
            Value::Function => "function",
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Value::Str(value) => !value.is_empty(),
            Value::Bool(value) => *value,
            Value::Undefined => false,
            Value::Object | Value::Function => true,
        }
    }

    /// `===` (strict) or `==` between two values, when the result is known.
    fn equals(&self, other: &Value, strict: bool) -> Option<bool> {
        match (self, other) {
            (Value::Object | Value::Function, _) | (_, Value::Object | Value::Function) => None,
            (Value::Str(a), Value::Str(b)) => Some(a == b),
            (Value::Bool(a), Value::Bool(b)) => Some(a == b),
            (Value::Undefined, Value::Undefined) => Some(true),
            // `undefined` loosely equals only `null` and `undefined`.
            (Value::Undefined, _) | (_, Value::Undefined) => Some(false),
            // A string against a boolean coerces both to numbers.
            _ if strict => Some(false),
            _ => None,
        }
    }
}

/// Whether evaluating `expr` has no effect other than throwing for an
/// undeclared name: literals, `this`, name reads, function expressions, and
/// `typeof` / `!` / `void` / comparison / logical / conditional combinations
/// of those. A member read can run a getter and is not accepted.
fn is_effect_free(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(paren) => is_effect_free(&paren.expr),
        Expr::Lit(_) | Expr::This(_) | Expr::Ident(_) | Expr::Fn(_) | Expr::Arrow(_) => true,
        Expr::Tpl(tpl) => tpl.exprs.is_empty(),
        Expr::Object(object) => object.props.is_empty(),
        Expr::Array(array) => array.elems.is_empty(),
        Expr::Unary(unary) => {
            matches!(unary.op, UnaryOp::TypeOf | UnaryOp::Bang | UnaryOp::Void)
                && is_effect_free(&unary.arg)
        }
        Expr::Bin(bin) => match bin.op {
            BinaryOp::EqEqEq | BinaryOp::NotEqEq | BinaryOp::LogicalAnd | BinaryOp::LogicalOr => {
                is_effect_free(&bin.left) && is_effect_free(&bin.right)
            }
            // A loose or relational comparison converts an object operand
            // through `valueOf` / `toString`; between primitives it does not.
            BinaryOp::EqEq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::LtEq
            | BinaryOp::Gt
            | BinaryOp::GtEq => is_primitive_operand(&bin.left) && is_primitive_operand(&bin.right),
            _ => false,
        },
        Expr::Cond(cond) => {
            is_effect_free(&cond.test) && is_effect_free(&cond.cons) && is_effect_free(&cond.alt)
        }
        _ => false,
    }
}

fn is_primitive_operand(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Lit(_) => true,
        Expr::Tpl(tpl) => tpl.exprs.is_empty(),
        Expr::Unary(unary) if unary.op == UnaryOp::TypeOf => is_effect_free(&unary.arg),
        _ => false,
    }
}

struct Env {
    /// Names the wrapper declares itself; their values are unknown until
    /// Node's branch assigns them.
    shadowed: HashSet<Atom>,
    /// Values the walk saw Node's branch give the wrapper's own variables
    /// (`var u = "undefined"`).
    locals: HashMap<Atom, Value>,
}

impl Env {
    /// The value of a global read under Node's CommonJS module scope. `None`
    /// for a name that is undeclared there (the read throws) or unknown.
    fn global_value(&self, name: &str) -> Option<Value> {
        if self.shadowed.contains(&Atom::from(name)) {
            return None;
        }
        match name {
            "module" | "exports" | "globalThis" | "global" => Some(Value::Object),
            "require" => Some(Value::Function),
            "undefined" => Some(Value::Undefined),
            _ => None,
        }
    }

    /// `typeof name` for a free name: unlike a read, an undeclared name gives
    /// `"undefined"` instead of throwing.
    fn global_type_of(&self, name: &str) -> Option<Value> {
        if self.shadowed.contains(&Atom::from(name)) {
            return None;
        }
        if let Some(value) = self.global_value(name) {
            return Some(Value::Str(value.type_of().into()));
        }
        match name {
            // Browser and AMD-loader globals a UMD wrapper tests; Node defines
            // none of them.
            "define" | "window" | "self" | "document" | "navigator" => {
                Some(Value::Str("undefined".into()))
            }
            _ => None,
        }
    }

    fn eval(&self, expr: &Expr) -> Option<Value> {
        match expr {
            Expr::Paren(paren) => self.eval(&paren.expr),
            Expr::Lit(Lit::Str(value)) => Some(Value::Str(value.value.as_str()?.into())),
            Expr::Lit(Lit::Bool(value)) => Some(Value::Bool(value.value)),
            Expr::Tpl(tpl) if tpl.exprs.is_empty() => {
                let [quasi] = tpl.quasis.as_slice() else {
                    return None;
                };
                Some(Value::Str(quasi.cooked.as_ref()?.as_str()?.into()))
            }
            Expr::Ident(ident) => match self.locals.get(&ident.sym) {
                Some(value) => Some(value.clone()),
                None => self.global_value(&ident.sym),
            },
            Expr::Member(member) => self.member_value(member),
            Expr::Unary(unary) => match unary.op {
                UnaryOp::TypeOf => match strip_parens(&unary.arg) {
                    Expr::Ident(ident) if !self.locals.contains_key(&ident.sym) => {
                        self.global_type_of(&ident.sym)
                    }
                    arg => Some(Value::Str(self.eval(arg)?.type_of().into())),
                },
                UnaryOp::Bang => Some(Value::Bool(!self.eval(&unary.arg)?.truthy())),
                UnaryOp::Void => {
                    self.eval(&unary.arg)?;
                    Some(Value::Undefined)
                }
                _ => None,
            },
            Expr::Bin(bin) => match bin.op {
                BinaryOp::LogicalAnd | BinaryOp::LogicalOr => {
                    let left = self.eval(&bin.left)?;
                    if left.truthy() == matches!(bin.op, BinaryOp::LogicalAnd) {
                        self.eval(&bin.right)
                    } else {
                        Some(left)
                    }
                }
                BinaryOp::EqEqEq | BinaryOp::NotEqEq | BinaryOp::EqEq | BinaryOp::NotEq => {
                    let left = self.eval(&bin.left)?;
                    let right = self.eval(&bin.right)?;
                    let strict = matches!(bin.op, BinaryOp::EqEqEq | BinaryOp::NotEqEq);
                    let equal = left.equals(&right, strict)?;
                    Some(Value::Bool(
                        equal == matches!(bin.op, BinaryOp::EqEqEq | BinaryOp::EqEq),
                    ))
                }
                // esbuild's minifier writes `typeof x !== "undefined"` as
                // `typeof x < "u"`: of the `typeof` results only "undefined"
                // sorts at or after "u".
                BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => {
                    let (Value::Str(left), Value::Str(right)) =
                        (self.eval(&bin.left)?, self.eval(&bin.right)?)
                    else {
                        return None;
                    };
                    // UTF-16 code unit order, as JavaScript compares strings.
                    let order = left.encode_utf16().cmp(right.encode_utf16());
                    Some(Value::Bool(match bin.op {
                        BinaryOp::Lt => order.is_lt(),
                        BinaryOp::LtEq => order.is_le(),
                        BinaryOp::Gt => order.is_gt(),
                        _ => order.is_ge(),
                    }))
                }
                _ => None,
            },
            Expr::Cond(cond) => {
                if self.eval(&cond.test)?.truthy() {
                    self.eval(&cond.cons)
                } else {
                    self.eval(&cond.alt)
                }
            }
            Expr::Seq(seq) => {
                let mut last = None;
                for expr in &seq.exprs {
                    last = Some(self.eval(expr)?);
                }
                last
            }
            _ => None,
        }
    }

    /// `module.exports` is an object; any named property of the fresh
    /// exports object (`exports.x`, `module.exports.x`) is `undefined` while
    /// the wrapper runs, because the file's only statement is the wrapper.
    fn member_value(&self, member: &MemberExpr) -> Option<Value> {
        let MemberProp::Ident(prop) = &member.prop else {
            return None;
        };
        match strip_parens(&member.obj) {
            Expr::Ident(obj) if obj.sym == "module" && self.global_value("module").is_some() => {
                (prop.sym == "exports").then_some(Value::Object)
            }
            Expr::Ident(obj) if obj.sym == "exports" && self.global_value("exports").is_some() => {
                Some(Value::Undefined)
            }
            Expr::Member(inner) if self.is_module_exports(inner) => Some(Value::Undefined),
            _ => None,
        }
    }

    fn is_module_exports(&self, member: &MemberExpr) -> bool {
        matches!(strip_parens(&member.obj), Expr::Ident(obj) if obj.sym == "module")
            && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "exports")
            && self.global_value("module").is_some()
    }

    fn global_ident_is(&self, expr: &Expr, name: &str) -> bool {
        matches!(strip_parens(expr), Expr::Ident(ident) if ident.sym == name)
            && self.global_value(name).is_some()
    }

    fn factory_arg(&self, expr: &Expr) -> Option<UmdArg> {
        let expr = strip_parens(expr);
        if self.global_ident_is(expr, "exports") {
            return Some(UmdArg::Exports);
        }
        if self.global_ident_is(expr, "module") {
            return Some(UmdArg::Module);
        }
        if self.global_ident_is(expr, "require") {
            return Some(UmdArg::Require);
        }
        let Expr::Call(call) = expr else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        if !self.global_ident_is(callee, "require") {
            return None;
        }
        let [arg] = call.args.as_slice() else {
            return None;
        };
        if arg.spread.is_some() {
            return None;
        }
        match strip_parens(&arg.expr) {
            Expr::Lit(Lit::Str(value)) => Some(UmdArg::RequireCall(value.value.as_str()?.into())),
            _ => None,
        }
    }
}

/// Follows the wrapper body down the branches Node takes and records the one
/// factory call on that path. Any other effect fails the walk.
struct BranchWalk<'a> {
    env: Env,
    factory_sym: Atom,
    found: Option<(&'a CallExpr, UmdResult)>,
}

impl<'a> BranchWalk<'a> {
    fn stmt(&mut self, stmt: &'a Stmt) -> Option<()> {
        match stmt {
            Stmt::Empty(_) => Some(()),
            Stmt::Block(block) => {
                for stmt in &block.stmts {
                    self.stmt(stmt)?;
                }
                Some(())
            }
            Stmt::If(if_stmt) => {
                if self.env.eval(&if_stmt.test)?.truthy() {
                    self.stmt(&if_stmt.cons)
                } else if let Some(alt) = &if_stmt.alt {
                    self.stmt(alt)
                } else {
                    Some(())
                }
            }
            Stmt::Expr(expr_stmt) => self.effect(&expr_stmt.expr),
            // A declaration on Node's branch gives the wrapper's variable a
            // value the later tests can read (`var u = "undefined"; if (typeof
            // module != u) …`). `var x;` keeps an earlier value.
            Stmt::Decl(Decl::Var(var)) => {
                for decl in &var.decls {
                    let Pat::Ident(binding) = &decl.name else {
                        return None;
                    };
                    match &decl.init {
                        Some(init) => {
                            let value = self.env.eval(init)?;
                            self.env.locals.insert(binding.sym.clone(), value);
                        }
                        None if var.kind == VarDeclKind::Var => {
                            self.env
                                .locals
                                .entry(binding.sym.clone())
                                .or_insert(Value::Undefined);
                        }
                        None => {
                            self.env
                                .locals
                                .insert(binding.sym.clone(), Value::Undefined);
                        }
                    }
                }
                Some(())
            }
            _ => None,
        }
    }

    fn effect(&mut self, expr: &'a Expr) -> Option<()> {
        match expr {
            Expr::Paren(paren) => self.effect(&paren.expr),
            Expr::Cond(cond) => {
                if self.env.eval(&cond.test)?.truthy() {
                    self.effect(&cond.cons)
                } else {
                    self.effect(&cond.alt)
                }
            }
            Expr::Bin(bin) if matches!(bin.op, BinaryOp::LogicalAnd | BinaryOp::LogicalOr) => {
                let left = self.env.eval(&bin.left)?;
                if left.truthy() == matches!(bin.op, BinaryOp::LogicalAnd) {
                    self.effect(&bin.right)
                } else {
                    Some(())
                }
            }
            Expr::Seq(seq) => {
                for expr in &seq.exprs {
                    self.effect(expr)?;
                }
                Some(())
            }
            Expr::Unary(unary) if matches!(unary.op, UnaryOp::Bang | UnaryOp::Void) => {
                self.effect(&unary.arg)
            }
            Expr::Call(call) if self.is_factory_call(call) => {
                self.record(call, UmdResult::Discarded)
            }
            Expr::Assign(assign) if assign.op == AssignOp::Assign => {
                let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assign.left else {
                    return None;
                };
                if !self.env.is_module_exports(target) {
                    return None;
                }
                let Expr::Call(call) = strip_parens(&assign.right) else {
                    return None;
                };
                if !self.is_factory_call(call) {
                    return None;
                }
                self.record(call, UmdResult::ModuleExports)
            }
            expr => self.env.eval(expr).map(|_| ()),
        }
    }

    fn is_factory_call(&self, call: &CallExpr) -> bool {
        matches!(&call.callee, Callee::Expr(callee)
            if matches!(strip_parens(callee), Expr::Ident(ident) if ident.sym == self.factory_sym))
    }

    fn record(&mut self, call: &'a CallExpr, result: UmdResult) -> Option<()> {
        if self.found.is_some() {
            return None;
        }
        self.found = Some((call, result));
        Some(())
    }
}

/// Names a function body declares in its own scope: hoisted `var` and
/// function declarations at any block depth, and lexical declarations at
/// the top level. Nested functions and classes are not entered.
#[derive(Default)]
pub(super) struct HoistedNames {
    pub names: HashSet<Atom>,
    pub function_names: HashSet<Atom>,
    depth: usize,
}

impl HoistedNames {
    pub(super) fn of_stmts(stmts: &[Stmt]) -> Self {
        let mut names = Self::default();
        for stmt in stmts {
            stmt.visit_with(&mut names);
        }
        names
    }

    fn add_pat(&mut self, pat: &Pat) {
        let mut ids = Vec::new();
        collect_pat_names(pat, &mut ids);
        self.names.extend(ids);
    }
}

fn collect_pat_names(pat: &Pat, out: &mut Vec<Atom>) {
    match pat {
        Pat::Ident(binding) => out.push(binding.sym.clone()),
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_names(elem, out);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                        collect_pat_names(&kv.value, out)
                    }
                    swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                        out.push(assign.key.sym.clone())
                    }
                    swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                        collect_pat_names(&rest.arg, out)
                    }
                }
            }
        }
        Pat::Rest(rest) => collect_pat_names(&rest.arg, out),
        Pat::Assign(assign) => collect_pat_names(&assign.left, out),
        Pat::Expr(_) | Pat::Invalid(_) => {}
    }
}

impl Visit for HoistedNames {
    fn visit_var_decl(&mut self, var: &VarDecl) {
        if var.kind == VarDeclKind::Var || self.depth == 0 {
            for decl in &var.decls {
                self.add_pat(&decl.name);
            }
        }
        // Initializers can hold arrow functions or classes, which are not
        // entered; anything else declares nothing.
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.names.insert(decl.ident.sym.clone());
        self.function_names.insert(decl.ident.sym.clone());
    }

    fn visit_class_decl(&mut self, decl: &swc_core::ecma::ast::ClassDecl) {
        if self.depth == 0 {
            self.names.insert(decl.ident.sym.clone());
        }
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_class(&mut self, _: &swc_core::ecma::ast::Class) {}

    fn visit_expr(&mut self, _: &Expr) {}

    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Decl(_) | Stmt::Expr(_) => stmt.visit_children_with(self),
            _ => {
                self.depth += 1;
                stmt.visit_children_with(self);
                self.depth -= 1;
            }
        }
    }
}
