//! Class-member helpers of Babel, swc, and esbuild, recognized by body.
//!
//! tsc's `__classPrivateFieldGet/Set` are found in `transpiler_helper_utils`.
//! The other producers lower `#x` through helpers with different argument
//! orders and storage, so each is matched here by what its body does with its
//! parameters, never by its name:
//!
//! | Producer | init | get | set | method |
//! |---|---|---|---|---|
//! | Babel 7.24+ | `init(this, map, v)` | `get(map, this)` | `set(map, this, v)` | `_assertClassBrand(brand, this, fn)` |
//! | swc, older Babel | `init(this, map, {writable: true, value: v})` or `map.set(this, …)` | `get(this, map)` | `set(this, map, v)` | `get(this, brand, fn)` |
//! | esbuild | `__privateAdd(this, map, v)` | `__privateGet(this, map)` | `__privateSet(this, map, v)` | `__privateMethod(this, brand, fn)` |
//!
//! swc and older Babel store a descriptor in the map and read its `value`
//! ([`PrivateStorage::Descriptor`]); the others store the value itself.
//! Sub-helpers (brand checks, redeclaration checks, descriptor accessors) are
//! followed through a binding or an inline function, the form Terser
//! leaves for a single-use helper.
//!
//! esbuild's public-field helper, `__publicField(this, key, value)` over
//! `__defNormalProp`, is matched here too
//! ([`collect_esbuild_public_field_helpers`]).

use crate::collections::{HashMap, HashSet};

use swc_core::common::Mark;
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, BinaryOp, CallExpr, Callee, Decl, Expr, ExprOrSpread, ExprStmt,
    Function, Lit, MemberExpr, MemberProp, Module, Pat, ReturnStmt, Stmt, UnaryOp,
};

use super::helper_matcher::binding_key;
use super::transpiler_helper_utils::BindingKey;
use crate::utils::paren::strip_parens;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivateStorage {
    /// The map holds the field value.
    Raw,
    /// The map holds `{ writable: true, value }`; reads and writes go through
    /// its `value`.
    Descriptor,
}

/// A recognized helper and where its call passes the receiver, the backing
/// map, and the value. Every call must pass exactly `arity` arguments: an
/// esbuild getter or setter argument would mean an accessor, not a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoweredPrivateHelper {
    /// `(receiver, map, value)`: defines the field on a fresh receiver. The
    /// value is the descriptor when the map's storage is a descriptor.
    Init,
    /// `(receiver, set)`: adds the private-method brand to a fresh receiver
    /// (Babel's `_classPrivateMethodInitSpec`, swc's
    /// `_class_private_method_init`).
    AddBrand,
    Get {
        receiver: usize,
        map: usize,
        arity: usize,
        storage: PrivateStorage,
    },
    Set {
        receiver: usize,
        map: usize,
        value: usize,
        arity: usize,
        storage: PrivateStorage,
    },
    /// `(receiver, brand, method)`, exactly three arguments: returns `method`
    /// when the brand set holds the receiver and throws otherwise. esbuild's
    /// `__privateMethod`, swc's and older Babel's `_classPrivateMethodGet`,
    /// and Babel's `_assertClassBrand` called with a value.
    MethodGet {
        receiver: usize,
        brand: usize,
        method: usize,
    },
}

/// The recognized helpers of a module, and every helper binding they reach.
#[derive(Default)]
pub(crate) struct LoweredPrivateHelpers {
    pub(crate) roles: HashMap<BindingKey, LoweredPrivateHelper>,
    /// The helpers and the sub-helper bindings they call, for removal once
    /// no call is left.
    pub(crate) bindings: HashSet<BindingKey>,
}

/// Every function bound to a never-written binding, by a declaration or a
/// declarator, in any statement list; and the never-written
/// `x = Object.defineProperty` aliases.
fn collect_function_views(
    module: &Module,
    unresolved_mark: Mark,
) -> (HashMap<BindingKey, FnView>, HashSet<BindingKey>) {
    use swc_core::ecma::visit::{Visit, VisitWith};
    struct Collector<'a> {
        written: &'a HashSet<BindingKey>,
        unresolved_mark: Mark,
        functions: HashMap<BindingKey, FnView>,
        define_property_aliases: HashSet<BindingKey>,
    }
    impl Visit for Collector<'_> {
        fn visit_fn_decl(&mut self, fn_decl: &swc_core::ecma::ast::FnDecl) {
            let key = binding_key(&fn_decl.ident);
            if !self.written.contains(&key) {
                if let Some(view) = FnView::from_function(&fn_decl.function) {
                    self.functions.insert(key, view);
                }
            }
            fn_decl.visit_children_with(self);
        }
        fn visit_var_declarator(&mut self, declarator: &swc_core::ecma::ast::VarDeclarator) {
            if let Pat::Ident(binding) = &declarator.name {
                let key = binding_key(&binding.id);
                if !self.written.contains(&key) {
                    if let Some(init) = declarator.init.as_deref() {
                        if let Some(view) = FnView::from_expr(init) {
                            self.functions.insert(key, view);
                        } else if is_object_define_property(init, self.unresolved_mark) {
                            self.define_property_aliases.insert(key);
                        }
                    }
                }
            }
            declarator.visit_children_with(self);
        }
    }
    let written =
        crate::analysis::binding_uses::BindingUseIndex::collect_direct_write_bindings(module);
    let mut collector = Collector {
        written: &written,
        unresolved_mark,
        functions: HashMap::default(),
        define_property_aliases: HashSet::default(),
    };
    module.visit_with(&mut collector);
    (collector.functions, collector.define_property_aliases)
}

/// Helper bindings that are never written, by role. Helpers are found in
/// any statement list: a UMD wrapper or an unsplit module factory holds them
/// in a function body.
pub(crate) fn collect_lowered_private_helpers(
    module: &Module,
    unresolved_mark: Mark,
) -> LoweredPrivateHelpers {
    use swc_core::ecma::visit::{Visit, VisitWith};
    let (functions, _) = collect_function_views(module, unresolved_mark);
    if functions.is_empty() {
        return LoweredPrivateHelpers::default();
    }
    let matcher = Matcher {
        functions: &functions,
        unresolved_mark,
    };
    let roles: HashMap<BindingKey, LoweredPrivateHelper> = functions
        .iter()
        .filter_map(|(key, view)| matcher.helper_kind(view).map(|kind| (key.clone(), kind)))
        .collect();

    struct Refs<'a> {
        functions: &'a HashMap<BindingKey, FnView>,
        found: Vec<BindingKey>,
    }
    impl Visit for Refs<'_> {
        fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
            let key = binding_key(ident);
            if self.functions.contains_key(&key) {
                self.found.push(key);
            }
        }
    }
    let mut bindings: HashSet<BindingKey> = roles.keys().cloned().collect();
    let mut frontier: Vec<BindingKey> = bindings.iter().cloned().collect();
    while let Some(key) = frontier.pop() {
        let mut refs = Refs {
            functions: &functions,
            found: Vec::new(),
        };
        functions[&key].body.visit_with(&mut refs);
        for dependency in refs.found {
            if bindings.insert(dependency.clone()) {
                frontier.push(dependency);
            }
        }
    }
    LoweredPrivateHelpers { roles, bindings }
}

/// esbuild's `__publicField` helpers of a module.
#[derive(Default)]
pub(crate) struct PublicFieldHelpers {
    /// `__publicField(obj, key, value)` bindings: called as a statement with a
    /// string key, it defines a writable, enumerable, configurable data
    /// property, as a class field does.
    pub(crate) helpers: HashSet<BindingKey>,
    /// The helpers, the `__defNormalProp` helpers they call, and the
    /// `Object.defineProperty` aliases those call, for removal once no call
    /// is left.
    pub(crate) bindings: HashSet<BindingKey>,
}

/// `__publicField = (obj, key, value) => __defNormalProp(obj, typeof key !==
/// "symbol" ? key + "" : key, value)` over `__defNormalProp = (obj, key,
/// value) => key in obj ? __defProp(obj, key, { enumerable: true,
/// configurable: true, writable: true, value }) : obj[key] = value`, matched
/// by body in any statement list, never by name.
pub(crate) fn collect_esbuild_public_field_helpers(
    module: &Module,
    unresolved_mark: Mark,
) -> PublicFieldHelpers {
    let (functions, define_property_aliases) = collect_function_views(module, unresolved_mark);
    let is_define_property = |callee: &Expr| match strip_parens(callee) {
        Expr::Ident(id) => define_property_aliases.contains(&binding_key(id)),
        other => is_object_define_property(other, unresolved_mark),
    };
    let mut result = PublicFieldHelpers::default();
    for (key, view) in &functions {
        let Some(define) = public_field_define_callee(view) else {
            continue;
        };
        let Some(define_view) = functions.get(&define) else {
            continue;
        };
        let Some(define_property) = define_normal_prop_callee(define_view, &is_define_property)
        else {
            continue;
        };
        result.helpers.insert(key.clone());
        result.bindings.insert(key.clone());
        result.bindings.insert(define);
        if let Expr::Ident(alias) = strip_parens(define_property) {
            result.bindings.insert(binding_key(alias));
        }
    }
    result
}

/// `(obj, key, value) => define(obj, toKey(key), value)`: `define`.
fn public_field_define_callee(f: &FnView) -> Option<BindingKey> {
    if f.params.len() != 3 {
        return None;
    }
    let [Stmt::Return(ReturnStmt { arg: Some(ret), .. })] = f.body.as_slice() else {
        return None;
    };
    let Expr::Call(call) = strip_parens(ret) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(define) = strip_parens(callee) else {
        return None;
    };
    let (o, k, v) = (f.param(0), f.param(1), f.param(2));
    let args = plain_args(call)?;
    let [obj, key, value] = args.as_slice() else {
        return None;
    };
    (is_param(obj, o) && is_param(value, v) && is_symbol_or_string_key(key, k))
        .then(|| binding_key(define))
}

/// `typeof key !== "symbol" ? key + "" : key`, with the string conversion
/// also as `` `${key}` `` and the test in either order or negated.
fn is_symbol_or_string_key(expr: &Expr, k: &BindingKey) -> bool {
    let Expr::Cond(cond) = strip_parens(expr) else {
        return false;
    };
    let Expr::Bin(test) = strip_parens(&cond.test) else {
        return false;
    };
    let is_typeof_key = |expr: &Expr| {
        matches!(strip_parens(expr), Expr::Unary(unary)
            if unary.op == UnaryOp::TypeOf && is_param(&unary.arg, k))
    };
    let is_symbol_str = |expr: &Expr| matches!(strip_parens(expr), Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some("symbol"));
    if !((is_typeof_key(&test.left) && is_symbol_str(&test.right))
        || (is_symbol_str(&test.left) && is_typeof_key(&test.right)))
    {
        return false;
    }
    let is_string_conversion = |expr: &Expr| match strip_parens(expr) {
        Expr::Bin(bin) => {
            bin.op == BinaryOp::Add
                && is_param(&bin.left, k)
                && matches!(strip_parens(&bin.right), Expr::Lit(Lit::Str(s)) if s.value.is_empty())
        }
        Expr::Tpl(tpl) => {
            matches!(tpl.exprs.as_slice(), [only] if is_param(only, k))
                && tpl.quasis.iter().all(|quasi| {
                    quasi
                        .cooked
                        .as_ref()
                        .is_some_and(|cooked| cooked.is_empty())
                })
        }
        _ => false,
    };
    match test.op {
        BinaryOp::NotEqEq | BinaryOp::NotEq => {
            is_string_conversion(&cond.cons) && is_param(&cond.alt, k)
        }
        BinaryOp::EqEqEq | BinaryOp::EqEq => {
            is_param(&cond.cons, k) && is_string_conversion(&cond.alt)
        }
        _ => false,
    }
}

/// `(obj, key, value) => key in obj ? defineProperty(obj, key, { enumerable:
/// true, configurable: true, writable: true, value }) : obj[key] = value`, or
/// the same as `if (key in obj) return …; return obj[key] = value`: the
/// `defineProperty` callee.
fn define_normal_prop_callee<'a>(
    f: &'a FnView,
    is_define_property: &dyn Fn(&Expr) -> bool,
) -> Option<&'a Expr> {
    if f.params.len() != 3 {
        return None;
    }
    let (o, k, v) = (f.param(0), f.param(1), f.param(2));
    let (test, define, assign) = match f.body.as_slice() {
        [Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => {
            let Expr::Cond(cond) = strip_parens(ret) else {
                return None;
            };
            (&*cond.test, &*cond.cons, &*cond.alt)
        }
        [Stmt::If(check), Stmt::Return(ReturnStmt {
            arg: Some(assign), ..
        })] if check.alt.is_none() => (&*check.test, single_return(&check.cons)?, &**assign),
        _ => return None,
    };
    let is_in_test = matches!(strip_parens(test), Expr::Bin(bin)
        if bin.op == BinaryOp::In && is_param(&bin.left, k) && is_param(&bin.right, o));
    let is_fallback_assign = matches!(strip_parens(assign), Expr::Assign(assign)
        if assign.op == swc_core::ecma::ast::AssignOp::Assign
            && matches!(&assign.left, swc_core::ecma::ast::AssignTarget::Simple(
                swc_core::ecma::ast::SimpleAssignTarget::Member(member))
                if is_param(&member.obj, o)
                    && matches!(&member.prop, MemberProp::Computed(prop) if is_param(&prop.expr, k)))
            && is_param(&assign.right, v));
    if !is_in_test || !is_fallback_assign {
        return None;
    }
    let Expr::Call(call) = strip_parens(define) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let args = plain_args(call)?;
    let [obj, key, descriptor] = args.as_slice() else {
        return None;
    };
    (is_define_property(callee)
        && is_param(obj, o)
        && is_param(key, k)
        && is_data_descriptor_of(descriptor, v))
    .then_some(&**callee)
}

/// `{ enumerable: true, configurable: true, writable: true, value }`, in any
/// order, `true` also as `!0`.
fn is_data_descriptor_of(expr: &Expr, v: &BindingKey) -> bool {
    use swc_core::ecma::ast::{Prop, PropName, PropOrSpread};
    let Expr::Object(object) = strip_parens(expr) else {
        return false;
    };
    let is_true = |expr: &Expr| match strip_parens(expr) {
        Expr::Lit(Lit::Bool(b)) => b.value,
        Expr::Unary(unary) => {
            unary.op == UnaryOp::Bang
                && matches!(strip_parens(&unary.arg), Expr::Lit(Lit::Num(n)) if n.value == 0.0)
        }
        _ => false,
    };
    let mut seen = [false; 4];
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        let (name, value_is) = match prop.as_ref() {
            Prop::Shorthand(id) => (id.sym.as_ref(), binding_key(id) == *v),
            Prop::KeyValue(kv) => {
                let PropName::Ident(name) = &kv.key else {
                    return false;
                };
                let name = name.sym.as_ref();
                let ok = if name == "value" {
                    is_param(&kv.value, v)
                } else {
                    is_true(&kv.value)
                };
                (name, ok)
            }
            _ => return false,
        };
        let slot = match name {
            "enumerable" => 0,
            "configurable" => 1,
            "writable" => 2,
            "value" => 3,
            _ => return false,
        };
        if !value_is || std::mem::replace(&mut seen[slot], true) {
            return false;
        }
    }
    seen.iter().all(|seen| *seen)
}

/// `Object.defineProperty`, `Object` the global.
fn is_object_define_property(expr: &Expr, unresolved_mark: Mark) -> bool {
    matches!(strip_parens(expr), Expr::Member(member)
        if member_prop_is(member, "defineProperty")
            && matches!(member.obj.as_ref(), Expr::Ident(obj)
                if obj.sym.as_ref() == "Object" && obj.ctxt.outer() == unresolved_mark))
}

/// A helper function's simple parameters and its body, with comma
/// expressions in statement position split and an arrow's expression body
/// made a `return`.
struct FnView {
    params: Vec<BindingKey>,
    body: Vec<Stmt>,
    is_arrow: bool,
}

impl FnView {
    fn from_expr(expr: &Expr) -> Option<Self> {
        match strip_parens(expr) {
            Expr::Fn(fn_expr) => Self::from_function(&fn_expr.function),
            Expr::Arrow(arrow) => Self::from_arrow(arrow),
            _ => None,
        }
    }

    fn from_function(function: &Function) -> Option<Self> {
        if function.is_async || function.is_generator {
            return None;
        }
        let params = function
            .params
            .iter()
            .map(|param| match &param.pat {
                Pat::Ident(binding) => Some(binding_key(&binding.id)),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        let body = function.body.as_ref()?;
        Some(Self {
            params,
            body: flatten_stmts(&body.stmts)?,
            is_arrow: false,
        })
    }

    fn from_arrow(arrow: &ArrowExpr) -> Option<Self> {
        if arrow.is_async || arrow.is_generator {
            return None;
        }
        let params = arrow
            .params
            .iter()
            .map(|pat| match pat {
                Pat::Ident(binding) => Some(binding_key(&binding.id)),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        let body = match arrow.body.as_ref() {
            ArrowFunctionBody::FunctionBody(block) => flatten_stmts(&block.stmts)?,
            ArrowFunctionBody::Expr(expr) => flatten_stmts(&[Stmt::Return(ReturnStmt {
                span: Default::default(),
                arg: Some(expr.clone()),
            })])?,
        };
        Some(Self {
            params,
            body,
            is_arrow: true,
        })
    }

    fn param(&self, index: usize) -> &BindingKey {
        &self.params[index]
    }
}

/// Helper bodies are a few statements; anything longer is not one of them.
const MAX_HELPER_STMTS: usize = 4;

fn flatten_stmts(stmts: &[Stmt]) -> Option<Vec<Stmt>> {
    if stmts.len() > MAX_HELPER_STMTS {
        return None;
    }
    let mut out = Vec::new();
    for stmt in stmts {
        match stmt {
            Stmt::Expr(ExprStmt { expr, span }) => {
                if let Expr::Seq(seq) = strip_parens(expr) {
                    for expr in &seq.exprs {
                        out.push(Stmt::Expr(ExprStmt {
                            span: *span,
                            expr: expr.clone(),
                        }));
                    }
                } else {
                    out.push(stmt.clone());
                }
            }
            Stmt::Return(ReturnStmt {
                arg: Some(arg),
                span,
            }) => {
                if let Expr::Seq(seq) = strip_parens(arg) {
                    let (last, init) = seq.exprs.split_last()?;
                    for expr in init {
                        out.push(Stmt::Expr(ExprStmt {
                            span: *span,
                            expr: expr.clone(),
                        }));
                    }
                    out.push(Stmt::Return(ReturnStmt {
                        span: *span,
                        arg: Some(last.clone()),
                    }));
                } else {
                    out.push(stmt.clone());
                }
            }
            _ => out.push(stmt.clone()),
        }
    }
    (out.len() <= MAX_HELPER_STMTS * 2).then_some(out)
}

/// What a sub-helper does, for the helpers that call it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubHelper {
    /// `(message) => { throw … }`
    Thrower,
    /// `(obj, map) => { if (map.has(obj)) throw … }`
    RedeclarationCheck,
    /// Babel's `_assertClassBrand(map, receiver[, value])`: returns the
    /// receiver when the map holds it, throws otherwise.
    BabelBrand,
    /// `(receiver, map, action) => { if (!map.has(receiver)) throw …; return
    /// map.get(receiver) }`
    ExtractDescriptor,
    /// `(receiver, descriptor) => descriptor.get ? descriptor.get.call(receiver) : descriptor.value`
    ApplyDescriptorGet,
    /// `(receiver, descriptor, value)`: calls `descriptor.set`, or throws on a
    /// read-only descriptor, or writes `descriptor.value`.
    ApplyDescriptorSet,
    /// esbuild's `__accessCheck(obj, member, msg)`: `member.has(obj) || throw`.
    AccessCheck,
}

struct Matcher<'a> {
    functions: &'a HashMap<BindingKey, FnView>,
    unresolved_mark: Mark,
}

impl Matcher<'_> {
    fn helper_kind(&self, f: &FnView) -> Option<LoweredPrivateHelper> {
        match f.params.len() {
            2 => self
                .babel_get(f)
                .or_else(|| self.descriptor_get(f))
                .or_else(|| self.add_brand(f))
                .or_else(|| self.esbuild_get(f)),
            3 => self
                .init(f)
                .or_else(|| self.babel_set(f))
                .or_else(|| self.descriptor_set(f))
                .or_else(|| self.esbuild_add(f))
                .or_else(|| self.esbuild_get(f))
                .or_else(|| self.esbuild_set(f))
                .or_else(|| self.method_get(f))
                .or_else(|| self.babel_brand_method(f)),
            4 => self.esbuild_set(f),
            _ => None,
        }
    }

    /// Babel and swc: `(obj, map, value) { check(obj, map); map.set(obj, value) }`.
    fn init(&self, f: &FnView) -> Option<LoweredPrivateHelper> {
        let [Stmt::Expr(check), Stmt::Expr(store)] = f.body.as_slice() else {
            return None;
        };
        let (o, m, v) = (f.param(0), f.param(1), f.param(2));
        (self.sub_call_is(&check.expr, SubHelper::RedeclarationCheck, &[o, m])
            && is_method_call_of(&store.expr, m, "set", &[o, v]))
        .then_some(LoweredPrivateHelper::Init)
    }

    /// Babel and swc: `(obj, set) { check(obj, set); set.add(obj) }`.
    fn add_brand(&self, f: &FnView) -> Option<LoweredPrivateHelper> {
        let [Stmt::Expr(check), Stmt::Expr(add)] = f.body.as_slice() else {
            return None;
        };
        let (o, set) = (f.param(0), f.param(1));
        (self.sub_call_is(&check.expr, SubHelper::RedeclarationCheck, &[o, set])
            && is_method_call_of(&add.expr, set, "add", &[o]))
        .then_some(LoweredPrivateHelper::AddBrand)
    }

    /// esbuild's `__privateAdd(obj, member, value)`.
    fn esbuild_add(&self, f: &FnView) -> Option<LoweredPrivateHelper> {
        let (o, m, v) = (f.param(0), f.param(1), f.param(2));
        let is_redeclare_throw = |expr: &Expr| self.sub_call_is_any_args(expr, SubHelper::Thrower);
        let is_weak_set_test = |expr: &Expr| {
            matches!(strip_parens(expr), Expr::Bin(bin)
                if bin.op == BinaryOp::InstanceOf
                    && is_param(&bin.left, m)
                    && matches!(strip_parens(&bin.right), Expr::Ident(id)
                        if id.sym.as_ref() == "WeakSet" && id.ctxt.outer() == self.unresolved_mark))
        };
        let matches = match f.body.as_slice() {
            [Stmt::If(redeclared), Stmt::If(weak_set), Stmt::Return(ReturnStmt {
                arg: Some(store), ..
            })] => {
                redeclared.alt.is_none()
                    && weak_set.alt.is_none()
                    && is_method_call_of(&redeclared.test, m, "has", &[o])
                    && single_return(&redeclared.cons).is_some_and(is_redeclare_throw)
                    && is_weak_set_test(&weak_set.test)
                    && single_return(&weak_set.cons)
                        .is_some_and(|add| is_method_call_of(add, m, "add", &[o]))
                    && is_method_call_of(store, m, "set", &[o, v])
            }
            [Stmt::Return(ReturnStmt { arg: Some(arg), .. })] => {
                let Expr::Cond(redeclared) = strip_parens(arg) else {
                    return None;
                };
                let Expr::Cond(weak_set) = strip_parens(&redeclared.alt) else {
                    return None;
                };
                is_method_call_of(&redeclared.test, m, "has", &[o])
                    && is_redeclare_throw(&redeclared.cons)
                    && is_weak_set_test(&weak_set.test)
                    && is_method_call_of(&weak_set.cons, m, "add", &[o])
                    && is_method_call_of(&weak_set.alt, m, "set", &[o, v])
            }
            // Older esbuild throws inline and returns nothing:
            // `if (member.has(obj)) throw …; member instanceof WeakSet ?
            // member.add(obj) : member.set(obj, value)`.
            [Stmt::If(redeclared), store] => {
                let redeclared_throws = redeclared.alt.is_none()
                    && is_method_call_of(&redeclared.test, m, "has", &[o])
                    && is_throw_block(&redeclared.cons);
                let stores = match store {
                    Stmt::If(weak_set) => {
                        is_weak_set_test(&weak_set.test)
                            && single_expr(&weak_set.cons)
                                .is_some_and(|add| is_method_call_of(add, m, "add", &[o]))
                            && weak_set
                                .alt
                                .as_deref()
                                .and_then(single_expr)
                                .is_some_and(|set| is_method_call_of(set, m, "set", &[o, v]))
                    }
                    Stmt::Expr(store) => matches!(strip_parens(&store.expr), Expr::Cond(weak_set)
                        if is_weak_set_test(&weak_set.test)
                            && is_method_call_of(&weak_set.cons, m, "add", &[o])
                            && is_method_call_of(&weak_set.alt, m, "set", &[o, v])),
                    _ => false,
                };
                redeclared_throws && stores
            }
            _ => false,
        };
        matches.then_some(LoweredPrivateHelper::Init)
    }

    /// esbuild's `__privateMethod(obj, member, method)`, `__accessCheck(obj,
    /// member, msg); return method`, and swc's and older Babel's
    /// `_classPrivateMethodGet(receiver, set, fn)`, `if (!set.has(receiver))
    /// throw …; return fn`.
    fn method_get(&self, f: &FnView) -> Option<LoweredPrivateHelper> {
        let (o, m, method) = (f.param(0), f.param(1), f.param(2));
        let matches = match f.body.as_slice() {
            [Stmt::Expr(check), Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => {
                is_param(ret, method) && self.is_access_check(&check.expr, o, m)
            }
            [Stmt::If(check), Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => {
                is_param(ret, method)
                    && check.alt.is_none()
                    && is_negated(&check.test)
                        .is_some_and(|test| is_method_call_of(test, m, "has", &[o]))
                    && is_throw_block(&check.cons)
            }
            _ => false,
        };
        matches.then_some(LoweredPrivateHelper::MethodGet {
            receiver: 0,
            brand: 1,
            method: 2,
        })
    }

    /// Babel's `_assertClassBrand(brand, receiver, value)`: called with three
    /// arguments it returns `value` once the brand holds the receiver.
    fn babel_brand_method(&self, f: &FnView) -> Option<LoweredPrivateHelper> {
        self.babel_brand(f)?;
        let [Stmt::If(check), Stmt::Throw(_)] = f.body.as_slice() else {
            return None;
        };
        let (o, value) = (f.param(1), f.param(2));
        let returns_value = match block_stmts(&check.cons) {
            [Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => {
                matches!(strip_parens(ret), Expr::Cond(cond)
                    if is_fewer_than_three_arguments(&cond.test)
                        && is_param(&cond.cons, o)
                        && is_param(&cond.alt, value))
            }
            [Stmt::If(_), Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => is_param(ret, value),
            _ => false,
        };
        returns_value.then_some(LoweredPrivateHelper::MethodGet {
            receiver: 1,
            brand: 0,
            method: 2,
        })
    }

    /// Babel's `_classPrivateFieldGet2(map, receiver)`:
    /// `return map.get(_assertClassBrand(map, receiver))`.
    fn babel_get(&self, f: &FnView) -> Option<LoweredPrivateHelper> {
        let [Stmt::Return(ReturnStmt { arg: Some(arg), .. })] = f.body.as_slice() else {
            return None;
        };
        let (m, o) = (f.param(0), f.param(1));
        let args = method_call_args(arg, m, "get")?;
        let [brand] = args.as_slice() else {
            return None;
        };
        self.sub_call_is(brand, SubHelper::BabelBrand, &[m, o])
            .then_some(LoweredPrivateHelper::Get {
                receiver: 1,
                map: 0,
                arity: 2,
                storage: PrivateStorage::Raw,
            })
    }

    /// Babel's `_classPrivateFieldSet2(map, receiver, value)`:
    /// `map.set(_assertClassBrand(map, receiver), value); return value`.
    fn babel_set(&self, f: &FnView) -> Option<LoweredPrivateHelper> {
        let [Stmt::Expr(store), Stmt::Return(ReturnStmt { arg: Some(ret), .. })] =
            f.body.as_slice()
        else {
            return None;
        };
        let (m, o, v) = (f.param(0), f.param(1), f.param(2));
        let args = method_call_args(&store.expr, m, "set")?;
        let [brand, value] = args.as_slice() else {
            return None;
        };
        (self.sub_call_is(brand, SubHelper::BabelBrand, &[m, o])
            && is_param(value, v)
            && is_param(ret, v))
        .then_some(LoweredPrivateHelper::Set {
            receiver: 1,
            map: 0,
            value: 2,
            arity: 3,
            storage: PrivateStorage::Raw,
        })
    }

    /// swc's `_class_private_field_get(receiver, map)` (also Babel before
    /// 7.24): extract the descriptor, then read through it.
    fn descriptor_get(&self, f: &FnView) -> Option<LoweredPrivateHelper> {
        let (o, m) = (f.param(0), f.param(1));
        let matches = match f.body.as_slice() {
            [Stmt::Decl(Decl::Var(var)), Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => {
                let descriptor = self.descriptor_binding(var, o, m)?;
                self.sub_call_args_are(
                    ret,
                    SubHelper::ApplyDescriptorGet,
                    |args| matches!(args, [r, d] if is_param(r, o) && is_param(d, &descriptor)),
                )
            }
            [Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => self.sub_call_args_are(
                ret,
                SubHelper::ApplyDescriptorGet,
                |args| matches!(args, [r, d] if is_param(r, o) && self.is_extract_call(d, o, m)),
            ),
            _ => false,
        };
        matches.then_some(LoweredPrivateHelper::Get {
            receiver: 0,
            map: 1,
            arity: 2,
            storage: PrivateStorage::Descriptor,
        })
    }

    /// swc's `_class_private_field_set(receiver, map, value)`.
    fn descriptor_set(&self, f: &FnView) -> Option<LoweredPrivateHelper> {
        let (o, m, v) = (f.param(0), f.param(1), f.param(2));
        let matches = match f.body.as_slice() {
            [Stmt::Decl(Decl::Var(var)), Stmt::Expr(apply), Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => {
                let descriptor = self.descriptor_binding(var, o, m)?;
                is_param(ret, v)
                    && self.sub_call_args_are(&apply.expr, SubHelper::ApplyDescriptorSet, |args| {
                        matches!(args, [r, d, value]
                            if is_param(r, o) && is_param(d, &descriptor) && is_param(value, v))
                    })
            }
            [Stmt::Expr(apply), Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => {
                is_param(ret, v)
                    && self.sub_call_args_are(&apply.expr, SubHelper::ApplyDescriptorSet, |args| {
                        matches!(args, [r, d, value]
                            if is_param(r, o) && self.is_extract_call(d, o, m) && is_param(value, v))
                    })
            }
            _ => false,
        };
        matches.then_some(LoweredPrivateHelper::Set {
            receiver: 0,
            map: 1,
            value: 2,
            arity: 3,
            storage: PrivateStorage::Descriptor,
        })
    }

    /// `var descriptor = extract(receiver, map, action)`: the descriptor's binding.
    fn descriptor_binding(
        &self,
        var: &swc_core::ecma::ast::VarDecl,
        o: &BindingKey,
        m: &BindingKey,
    ) -> Option<BindingKey> {
        let [declarator] = var.decls.as_slice() else {
            return None;
        };
        let Pat::Ident(binding) = &declarator.name else {
            return None;
        };
        self.is_extract_call(declarator.init.as_deref()?, o, m)
            .then(|| binding_key(&binding.id))
    }

    fn is_extract_call(&self, expr: &Expr, o: &BindingKey, m: &BindingKey) -> bool {
        self.sub_call_args_are(
            expr,
            SubHelper::ExtractDescriptor,
            |args| matches!(args, [r, map, _action] if is_param(r, o) && is_param(map, m)),
        )
    }

    /// esbuild's `__privateGet(obj, member, getter)`. Field reads pass two
    /// arguments, so `getter` is undefined. A minifier drops the getter
    /// branch from a bundle without private accessors.
    fn esbuild_get(&self, f: &FnView) -> Option<LoweredPrivateHelper> {
        let (o, m) = (f.param(0), f.param(1));
        let is_read = |expr: &Expr| is_method_call_of(expr, m, "get", &[o]);
        if let [Stmt::Expr(check), Stmt::Return(ReturnStmt {
            arg: Some(read), ..
        })] = f.body.as_slice()
        {
            if self.is_access_check(&check.expr, o, m) && is_read(read) {
                return Some(LoweredPrivateHelper::Get {
                    receiver: 0,
                    map: 1,
                    arity: 2,
                    storage: PrivateStorage::Raw,
                });
            }
        }
        if f.params.len() != 3 {
            return None;
        }
        let g = f.param(2);
        let is_getter_call = |expr: &Expr| is_method_call_of(expr, g, "call", &[o]);
        let matches = match f.body.as_slice() {
            [Stmt::Expr(check), Stmt::If(getter), Stmt::Return(ReturnStmt {
                arg: Some(read), ..
            })] => {
                self.is_access_check(&check.expr, o, m)
                    && getter.alt.is_none()
                    && is_param(&getter.test, g)
                    && single_return(&getter.cons).is_some_and(is_getter_call)
                    && is_read(read)
            }
            [Stmt::Expr(check), Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => {
                self.is_access_check(&check.expr, o, m)
                    && matches!(strip_parens(ret), Expr::Cond(cond)
                        if is_param(&cond.test, g) && is_getter_call(&cond.cons) && is_read(&cond.alt))
            }
            _ => false,
        };
        matches.then_some(LoweredPrivateHelper::Get {
            receiver: 0,
            map: 1,
            arity: 2,
            storage: PrivateStorage::Raw,
        })
    }

    /// esbuild's `__privateSet(obj, member, value, setter)`. Field writes pass
    /// three arguments, so `setter` is undefined. A minifier drops the setter
    /// branch from a bundle without private accessors.
    fn esbuild_set(&self, f: &FnView) -> Option<LoweredPrivateHelper> {
        let (o, m, v) = (f.param(0), f.param(1), f.param(2));
        let is_write = |expr: &Expr| is_method_call_of(expr, m, "set", &[o, v]);
        let set_helper = LoweredPrivateHelper::Set {
            receiver: 0,
            map: 1,
            value: 2,
            arity: 3,
            storage: PrivateStorage::Raw,
        };
        if let [Stmt::Expr(check), Stmt::Expr(store), Stmt::Return(ReturnStmt { arg: Some(ret), .. })] =
            f.body.as_slice()
        {
            if self.is_access_check(&check.expr, o, m) && is_write(&store.expr) && is_param(ret, v)
            {
                return Some(set_helper);
            }
        }
        if f.params.len() != 4 {
            return None;
        }
        let s = f.param(3);
        let is_setter_call = |expr: &Expr| is_method_call_of(expr, s, "call", &[o, v]);
        let matches = match f.body.as_slice() {
            [Stmt::Expr(check), Stmt::If(setter), Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => {
                self.is_access_check(&check.expr, o, m)
                    && is_param(&setter.test, s)
                    && single_expr(&setter.cons).is_some_and(is_setter_call)
                    && setter
                        .alt
                        .as_deref()
                        .and_then(single_expr)
                        .is_some_and(is_write)
                    && is_param(ret, v)
            }
            [Stmt::Expr(check), Stmt::Expr(store), Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => {
                self.is_access_check(&check.expr, o, m)
                    && matches!(strip_parens(&store.expr), Expr::Cond(cond)
                        if is_param(&cond.test, s) && is_setter_call(&cond.cons) && is_write(&cond.alt))
                    && is_param(ret, v)
            }
            _ => false,
        };
        matches.then_some(set_helper)
    }

    fn is_access_check(&self, expr: &Expr, o: &BindingKey, m: &BindingKey) -> bool {
        self.sub_call_args_are(
            expr,
            SubHelper::AccessCheck,
            |args| matches!(args, [obj, member, _msg] if is_param(obj, o) && is_param(member, m)),
        )
    }

    fn sub_call_is(&self, expr: &Expr, kind: SubHelper, params: &[&BindingKey]) -> bool {
        self.sub_call_args_are(expr, kind, |args| {
            args.len() == params.len()
                && args
                    .iter()
                    .zip(params)
                    .all(|(arg, param)| is_param(arg, param))
        })
    }

    fn sub_call_is_any_args(&self, expr: &Expr, kind: SubHelper) -> bool {
        self.sub_call_args_are(expr, kind, |_| true)
    }

    fn sub_call_args_are(
        &self,
        expr: &Expr,
        kind: SubHelper,
        args_match: impl FnOnce(&[&Expr]) -> bool,
    ) -> bool {
        let Expr::Call(call) = strip_parens(expr) else {
            return false;
        };
        let Some(args) = plain_args(call) else {
            return false;
        };
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };
        self.sub_helper_kind(callee) == Some(kind) && args_match(&args)
    }

    fn sub_helper_kind(&self, callee: &Expr) -> Option<SubHelper> {
        let callee = strip_parens(callee);
        let owned;
        let view = match callee {
            Expr::Ident(id) => self.functions.get(&binding_key(id))?,
            Expr::Fn(_) | Expr::Arrow(_) => {
                owned = FnView::from_expr(callee)?;
                &owned
            }
            _ => return None,
        };
        self.classify_sub_helper(view)
    }

    fn classify_sub_helper(&self, f: &FnView) -> Option<SubHelper> {
        match (f.params.len(), f.body.as_slice()) {
            (1, [Stmt::Throw(_)]) => Some(SubHelper::Thrower),
            (2..=3, [Stmt::If(_), Stmt::Throw(_)]) if !f.is_arrow => self.babel_brand(f),
            (2, [Stmt::If(check)]) => {
                let (o, m) = (f.param(0), f.param(1));
                (check.alt.is_none()
                    && is_method_call_of(&check.test, m, "has", &[o])
                    && is_throw_block(&check.cons))
                .then_some(SubHelper::RedeclarationCheck)
                .or_else(|| self.apply_descriptor_get(f))
            }
            (2, _) => self.apply_descriptor_get(f),
            (3, [Stmt::If(check), Stmt::Return(ReturnStmt { arg: Some(ret), .. })]) => {
                let (o, m) = (f.param(0), f.param(1));
                (check.alt.is_none()
                    && is_negated(&check.test)
                        .is_some_and(|test| is_method_call_of(test, m, "has", &[o]))
                    && is_throw_block(&check.cons)
                    && is_method_call_of(ret, m, "get", &[o]))
                .then_some(SubHelper::ExtractDescriptor)
            }
            // Older esbuild's `__accessCheck`: `if (!member.has(obj)) throw …`.
            (3, [Stmt::If(check)])
                if check.alt.is_none()
                    && is_negated(&check.test).is_some_and(|test| {
                        is_method_call_of(test, f.param(1), "has", &[f.param(0)])
                    })
                    && is_throw_block(&check.cons) =>
            {
                Some(SubHelper::AccessCheck)
            }
            (3, [Stmt::If(_)]) => self.apply_descriptor_set(f),
            (3, [Stmt::Return(ReturnStmt { arg: Some(ret), .. })]) => {
                let (o, m) = (f.param(0), f.param(1));
                matches!(strip_parens(ret), Expr::Bin(bin)
                    if bin.op == BinaryOp::LogicalOr
                        && is_method_call_of(&bin.left, m, "has", &[o])
                        && self.sub_call_is_any_args(&bin.right, SubHelper::Thrower))
                .then_some(SubHelper::AccessCheck)
            }
            _ => None,
        }
    }

    /// `if (typeof map == "function" ? map === receiver : map.has(receiver))
    /// return arguments.length < 3 ? receiver : value; throw …`. A map is an
    /// object, so a two-argument call returns the receiver or throws.
    fn babel_brand(&self, f: &FnView) -> Option<SubHelper> {
        let [Stmt::If(check), Stmt::Throw(_)] = f.body.as_slice() else {
            return None;
        };
        let (m, o) = (f.param(0), f.param(1));
        if check.alt.is_some() {
            return None;
        }
        let test_matches = is_method_call_of(&check.test, m, "has", &[o])
            || matches!(strip_parens(&check.test), Expr::Cond(cond)
                if is_typeof_function_test(&cond.test, m)
                    && matches!(strip_parens(&cond.cons), Expr::Bin(bin)
                        if bin.op == BinaryOp::EqEqEq && is_param(&bin.left, m) && is_param(&bin.right, o))
                    && is_method_call_of(&cond.alt, m, "has", &[o]));
        if !test_matches {
            return None;
        }
        let returns_receiver = match block_stmts(&check.cons) {
            [Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => {
                is_param(ret, o)
                    || matches!(strip_parens(ret), Expr::Cond(cond)
                        if is_fewer_than_three_arguments(&cond.test) && is_param(&cond.cons, o))
            }
            [Stmt::If(fewer), Stmt::Return(_)] => {
                fewer.alt.is_none()
                    && is_fewer_than_three_arguments(&fewer.test)
                    && single_return(&fewer.cons).is_some_and(|ret| is_param(ret, o))
            }
            _ => false,
        };
        returns_receiver.then_some(SubHelper::BabelBrand)
    }

    /// `(receiver, descriptor) { if (descriptor.get) return
    /// descriptor.get.call(receiver); return descriptor.value }`.
    fn apply_descriptor_get(&self, f: &FnView) -> Option<SubHelper> {
        let (r, d) = (f.param(0), f.param(1));
        let is_getter_test = |expr: &Expr| is_member_of(expr, d, "get");
        let is_getter_call = |expr: &Expr| {
            matches!(strip_parens(expr), Expr::Call(call)
                if matches!(&call.callee, Callee::Expr(callee)
                    if matches!(strip_parens(callee), Expr::Member(member)
                        if is_member_of(&member.obj, d, "get") && member_prop_is(member, "call")))
                    && plain_args(call).is_some_and(|args| matches!(args.as_slice(), [arg] if is_param(arg, r))))
        };
        let is_value = |expr: &Expr| is_member_of(expr, d, "value");
        let matches = match f.body.as_slice() {
            [Stmt::If(getter), Stmt::Return(ReturnStmt {
                arg: Some(value), ..
            })] => {
                getter.alt.is_none()
                    && is_getter_test(&getter.test)
                    && single_return(&getter.cons).is_some_and(is_getter_call)
                    && is_value(value)
            }
            [Stmt::Return(ReturnStmt { arg: Some(ret), .. })] => {
                matches!(strip_parens(ret), Expr::Cond(cond)
                    if is_getter_test(&cond.test) && is_getter_call(&cond.cons) && is_value(&cond.alt))
            }
            _ => false,
        };
        matches.then_some(SubHelper::ApplyDescriptorGet)
    }

    /// `(receiver, descriptor, value) { if (descriptor.set)
    /// descriptor.set.call(receiver, value); else { if (!descriptor.writable)
    /// throw …; descriptor.value = value } }`.
    fn apply_descriptor_set(&self, f: &FnView) -> Option<SubHelper> {
        let [Stmt::If(setter)] = f.body.as_slice() else {
            return None;
        };
        let (r, d, v) = (f.param(0), f.param(1), f.param(2));
        if !is_member_of(&setter.test, d, "set") {
            return None;
        }
        let setter_call = single_expr(&setter.cons)?;
        let Expr::Call(call) = strip_parens(setter_call) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let calls_setter = matches!(strip_parens(callee), Expr::Member(member)
            if is_member_of(&member.obj, d, "set") && member_prop_is(member, "call"))
            && plain_args(call).is_some_and(|args| {
                matches!(args.as_slice(), [recv, value] if is_param(recv, r) && is_param(value, v))
            });
        if !calls_setter {
            return None;
        }
        let alt = setter.alt.as_deref()?;
        let [Stmt::If(writable), Stmt::Expr(write)] = block_stmts(alt) else {
            return None;
        };
        let writes_value = matches!(strip_parens(&write.expr), Expr::Assign(assign)
            if assign.op == swc_core::ecma::ast::AssignOp::Assign
                && matches!(&assign.left, swc_core::ecma::ast::AssignTarget::Simple(
                    swc_core::ecma::ast::SimpleAssignTarget::Member(member))
                    if is_param(&member.obj, d) && member_prop_is(member, "value"))
                && is_param(&assign.right, v));
        (writable.alt.is_none()
            && is_negated(&writable.test).is_some_and(|test| is_member_of(test, d, "writable"))
            && is_throw_block(&writable.cons)
            && writes_value)
            .then_some(SubHelper::ApplyDescriptorSet)
    }
}

fn plain_args(call: &CallExpr) -> Option<Vec<&Expr>> {
    call.args
        .iter()
        .map(|ExprOrSpread { spread, expr }| spread.is_none().then_some(expr.as_ref()))
        .collect()
}

fn is_param(expr: &Expr, key: &BindingKey) -> bool {
    matches!(strip_parens(expr), Expr::Ident(id) if binding_key(id) == *key)
}

fn member_prop_is(member: &MemberExpr, name: &str) -> bool {
    matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == name)
}

/// `obj.name`, where `obj` is the parameter `key`.
fn is_member_of(expr: &Expr, key: &BindingKey, name: &str) -> bool {
    matches!(strip_parens(expr), Expr::Member(member)
        if is_param(&member.obj, key) && member_prop_is(member, name))
}

/// The arguments of `obj.method(…)`, where `obj` is the parameter `key`.
fn method_call_args<'a>(expr: &'a Expr, key: &BindingKey, method: &str) -> Option<Vec<&'a Expr>> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !is_member_of(callee, key, method) {
        return None;
    }
    plain_args(call)
}

/// `obj.method(params…)`, each argument exactly the given parameter.
fn is_method_call_of(expr: &Expr, key: &BindingKey, method: &str, params: &[&BindingKey]) -> bool {
    method_call_args(expr, key, method).is_some_and(|args| {
        args.len() == params.len()
            && args
                .iter()
                .zip(params)
                .all(|(arg, param)| is_param(arg, param))
    })
}

fn is_negated(expr: &Expr) -> Option<&Expr> {
    match strip_parens(expr) {
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => Some(&unary.arg),
        _ => None,
    }
}

fn is_typeof_function_test(expr: &Expr, key: &BindingKey) -> bool {
    let Expr::Bin(bin) = strip_parens(expr) else {
        return false;
    };
    if !matches!(bin.op, BinaryOp::EqEq | BinaryOp::EqEqEq) {
        return false;
    }
    let is_typeof = |expr: &Expr| {
        matches!(strip_parens(expr), Expr::Unary(unary)
            if unary.op == UnaryOp::TypeOf && is_param(&unary.arg, key))
    };
    let is_function_str = |expr: &Expr| matches!(strip_parens(expr), Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some("function"));
    (is_typeof(&bin.left) && is_function_str(&bin.right))
        || (is_function_str(&bin.left) && is_typeof(&bin.right))
}

/// `arguments.length < 3`
fn is_fewer_than_three_arguments(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Bin(bin)
        if bin.op == BinaryOp::Lt
            && matches!(strip_parens(&bin.left), Expr::Member(member)
                if matches!(member.obj.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "arguments")
                    && member_prop_is(member, "length"))
            && matches!(strip_parens(&bin.right), Expr::Lit(Lit::Num(n)) if n.value == 3.0))
}

fn block_stmts(stmt: &Stmt) -> &[Stmt] {
    match stmt {
        Stmt::Block(block) => &block.stmts,
        other => std::slice::from_ref(other),
    }
}

fn is_throw_block(stmt: &Stmt) -> bool {
    matches!(block_stmts(stmt), [Stmt::Throw(_)])
}

fn single_return(stmt: &Stmt) -> Option<&Expr> {
    match block_stmts(stmt) {
        [Stmt::Return(ReturnStmt { arg: Some(arg), .. })] => Some(arg),
        _ => None,
    }
}

fn single_expr(stmt: &Stmt) -> Option<&Expr> {
    match block_stmts(stmt) {
        [Stmt::Expr(expr)] => Some(&expr.expr),
        _ => None,
    }
}
