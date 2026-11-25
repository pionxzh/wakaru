//! Drops esbuild `keepNames` wrappers whose name the language gives anyway.
//!
//! esbuild's `--keep-names` declares a helper that sets a function's or
//! class's `name` and returns it (producer esbuild@0.28.0 --keep-names, esm
//! and iife formats; `--minify` mangles the helper and parameter names):
//!
//! ```js
//! var __defProp = Object.defineProperty;
//! var __name = (target, value) => __defProp(target, "name", { value, configurable: true });
//! const double = __name(function (x) { return x * 2; }, "double");
//! function quad(x) { return x * 4; }
//! __name(quad, "quad");
//! var Shape = class { static { __name(this, "Shape"); } };
//! ```
//!
//! A wrapper goes only when the value gets exactly the same `name` without
//! it: a named function or class expression whose own name is the string; an
//! anonymous one in a position that names it after the string (a declarator
//! or `=`/`||=`/`&&=`/`??=` assignment to that identifier, an object literal
//! or class field key, a destructuring default); a `helper(F, "F")`
//! statement after `function F` or `class F`; a
//! `static { helper(this, "C") }` block in a class named `C`. The helper
//! defines `name` with `value` and `configurable: true` only, so on a
//! function or class, which already owns a non-writable, non-enumerable,
//! configurable `name`, the property ends up with the same attributes either
//! way.
//!
//! Every other wrapper stays: a string the name does not match, a member
//! assignment (`o.x = helper(f, "x")` names nothing), a computed key, a
//! class with a static member that may be called `name` (it replaces the
//! language's name, and the wrapper puts the string back).
//!
//! The rule runs after every rename, so the binding name it compares is the
//! one the output prints: a wrapper whose string differs from the final name
//! stays and keeps the original name. A single-file decompile runs it at the
//! end of the rule range and, with a source map, again after source-map
//! names; unpack runs it once on each finished module (see the gate in
//! `pipeline.rs`).
//!
//! The helper is recognized by its body: an arrow of two parameters whose
//! body is `Object.defineProperty(target, "name", { value, configurable:
//! true })`, through a declared alias of `Object.defineProperty` or the
//! resolved global itself. The helper and the alias are declared once and
//! never written. Modules with a direct `eval` or `with` are left alone.

use crate::analysis::binding_uses::BindingUseIndex;
use crate::collections::{HashMap, HashSet};

use swc_core::common::Mark;
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, AssignExpr, AssignOp, AssignPat, AssignPatProp, AssignTarget,
    BindingIdent, Callee, Class, ClassDecl, ClassExpr, ClassMember, ClassProp, Decl, DefaultDecl,
    ExportDefaultDecl, ExportDefaultExpr, Expr, FnDecl, FnExpr, Ident, Key, KeyValueProp, Lit,
    MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, NewExpr, ParamOrTsParamProp, Pat,
    PrivateProp, Prop, PropName, PropOrSpread, SimpleAssignTarget, Stmt, TaggedTpl, UnaryExpr,
    UnaryOp, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::eval_utils::has_dynamic_scope_construct;
use super::helper_matcher::{
    binding_key, for_each_initialized_declarator, remove_unused_helper_declarations, BindingKey,
};
use crate::utils::paren::{strip_parens, strip_parens_owned};

pub struct UnEsbuildKeepNames {
    unresolved_mark: Mark,
}

impl UnEsbuildKeepNames {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for UnEsbuildKeepNames {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let (helpers, aliases) = keep_names_helper_candidates(module, self.unresolved_mark);
        if helpers.is_empty() || has_dynamic_scope_construct(module) {
            return;
        }
        let uses = BindingUseIndex::collect(module);
        let stable =
            |key: &BindingKey| uses.has_single_declaration(key) && !uses.has_direct_write(key);
        let helpers: HashSet<BindingKey> = helpers
            .into_iter()
            .filter(|(helper, alias)| stable(helper) && alias.as_ref().is_none_or(stable))
            .map(|(helper, _)| helper)
            .collect();
        if helpers.is_empty() {
            return;
        }
        let mut unwrapper = Unwrapper {
            helpers: &helpers,
            uses: &uses,
            changed: false,
        };
        module.visit_mut_with(&mut unwrapper);
        if unwrapper.changed {
            let mut candidates = helpers.clone();
            candidates.extend(aliases);
            remove_unused_helper_declarations(module, &candidates);
        }
    }
}

/// Every declarator shaped like the keep-names helper, with the
/// `Object.defineProperty` alias it calls (`None` for the global itself),
/// and the aliases those helpers call. Declaration and write counts are the
/// caller's check.
fn keep_names_helper_candidates(
    module: &Module,
    unresolved_mark: Mark,
) -> (Vec<(BindingKey, Option<BindingKey>)>, HashSet<BindingKey>) {
    let is_define_property = |expr: &Expr| {
        matches!(strip_parens(expr), Expr::Member(member)
            if matches!(&*member.obj, Expr::Ident(object)
                if object.sym == "Object" && object.ctxt.outer() == unresolved_mark)
            && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "defineProperty"))
    };
    let mut aliases = HashSet::default();
    for_each_initialized_declarator(module, |binding, init, _| {
        if is_define_property(init) {
            aliases.insert(binding_key(binding));
        }
    });
    let mut helpers = Vec::new();
    let mut used_aliases = HashSet::default();
    for_each_initialized_declarator(module, |binding, init, _| {
        let Expr::Arrow(arrow) = strip_parens(init) else {
            return;
        };
        let Some(alias) = helper_arrow_callee(arrow, &aliases, &is_define_property) else {
            return;
        };
        if let Some(alias) = &alias {
            used_aliases.insert(alias.clone());
        }
        helpers.push((binding_key(binding), alias));
    });
    (helpers, used_aliases)
}

/// `(target, value) => D(target, "name", { value, configurable: true })`:
/// `Some(None)` when `D` is `Object.defineProperty`, `Some(Some(alias))`
/// when it is one of `aliases`.
fn helper_arrow_callee(
    arrow: &ArrowExpr,
    aliases: &HashSet<BindingKey>,
    is_define_property: &impl Fn(&Expr) -> bool,
) -> Option<Option<BindingKey>> {
    if arrow.is_async || arrow.is_generator {
        return None;
    }
    let [Pat::Ident(target), Pat::Ident(value)] = arrow.params.as_slice() else {
        return None;
    };
    if target.to_id() == value.to_id() {
        return None;
    }
    let ArrowFunctionBody::Expr(body) = &*arrow.body else {
        return None;
    };
    let Expr::Call(call) = strip_parens(body) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let alias = match strip_parens(callee) {
        Expr::Ident(alias) if aliases.contains(&binding_key(alias)) => Some(binding_key(alias)),
        callee if is_define_property(callee) => None,
        _ => return None,
    };
    let [first, second, third] = call.args.as_slice() else {
        return None;
    };
    if [first, second, third]
        .iter()
        .any(|arg| arg.spread.is_some())
        || !matches!(&*first.expr, Expr::Ident(arg) if arg.to_id() == target.to_id())
        || !matches!(&*second.expr, Expr::Lit(Lit::Str(name)) if name.value == *"name")
        || !is_name_descriptor(&third.expr, &value.id)
    {
        return None;
    }
    Some(alias)
}

/// `{ value: <value>, configurable: true }`, in either order, with `value`
/// shorthand and `!0` for `true` allowed.
pub(crate) fn is_name_descriptor(expr: &Expr, value: &Ident) -> bool {
    let Expr::Object(object) = expr else {
        return false;
    };
    let mut has_value = false;
    let mut has_configurable = false;
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        match &**prop {
            Prop::Shorthand(ident) if ident.sym == "value" && ident.to_id() == value.to_id() => {
                has_value = true;
            }
            Prop::KeyValue(KeyValueProp {
                key: PropName::Ident(key),
                value: prop_value,
            }) if key.sym == "value" => {
                if !matches!(&**prop_value, Expr::Ident(ident) if ident.to_id() == value.to_id()) {
                    return false;
                }
                has_value = true;
            }
            Prop::KeyValue(KeyValueProp {
                key: PropName::Ident(key),
                value: prop_value,
            }) if key.sym == "configurable" => {
                let is_true = match &**prop_value {
                    Expr::Lit(Lit::Bool(flag)) => flag.value,
                    Expr::Unary(UnaryExpr {
                        op: UnaryOp::Bang,
                        arg,
                        ..
                    }) => matches!(&**arg, Expr::Lit(Lit::Num(num)) if num.value == 0.0),
                    _ => false,
                };
                if !is_true {
                    return false;
                }
                has_configurable = true;
            }
            _ => return false,
        }
    }
    has_value && has_configurable
}

/// `helper(value, "name")`: the value and the name.
fn wrapper_parts<'e>(helpers: &HashSet<BindingKey>, expr: &'e Expr) -> Option<(&'e Expr, &'e str)> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(callee) = &**callee else {
        return None;
    };
    if !helpers.contains(&binding_key(callee)) {
        return None;
    }
    let [value, name] = call.args.as_slice() else {
        return None;
    };
    if value.spread.is_some() || name.spread.is_some() {
        return None;
    }
    let Expr::Lit(Lit::Str(name)) = &*name.expr else {
        return None;
    };
    Some((&value.expr, name.value.as_str()?))
}

struct Unwrapper<'a> {
    helpers: &'a HashSet<BindingKey>,
    uses: &'a BindingUseIndex,
    changed: bool,
}

impl Unwrapper<'_> {
    /// Replaces `helper(value, "name")` with `value` when `value` is named
    /// `name` without the call: its own name, or `inferred`, the name its
    /// position gives an anonymous function or class.
    fn try_unwrap(&mut self, expr: &mut Expr, inferred: Option<&str>) {
        let Some((value, name)) = wrapper_parts(self.helpers, expr) else {
            return;
        };
        let inferred_matches = inferred == Some(name);
        let named = match strip_parens(value) {
            Expr::Fn(FnExpr { ident, .. }) => match ident {
                Some(ident) => ident.sym == *name,
                None => inferred_matches,
            },
            Expr::Arrow(_) => inferred_matches,
            // The class's static code runs before the helper does, and could
            // rewrite the `name` the helper then puts back.
            Expr::Class(ClassExpr { ident, class }) => {
                class_takes_plain_name(class)
                    && !class.body.iter().any(member_runs_static_code)
                    && match ident {
                        Some(ident) => ident.sym == *name,
                        None => inferred_matches,
                    }
            }
            _ => false,
        };
        if !named {
            return;
        }
        let Expr::Call(call) = std::mem::replace(expr, Expr::Invalid(Default::default())) else {
            unreachable!("wrapper_parts matched a call");
        };
        let value = call.args.into_iter().next().expect("two arguments");
        *expr = strip_parens_owned(*value.expr);
        self.changed = true;
    }

    /// A value in a position that names an anonymous function or class
    /// `name`.
    fn unwrap_named_value(&mut self, value: Option<&mut Expr>, name: &str) {
        let Some(value) = value else {
            return;
        };
        self.try_unwrap(value, Some(name));
        if let Expr::Class(ClassExpr { ident: None, class }) = strip_parens_mut(value) {
            self.drop_static_name_block(class, name);
        }
    }

    /// Removes `static { helper(this, "name") }` from a class whose own or
    /// inferred name is `name`. The block must be the first static code of
    /// the class: earlier static code could rewrite `name` before it runs.
    fn drop_static_name_block(&mut self, class: &mut Class, name: &str) {
        if !class_takes_plain_name(class) {
            return;
        }
        let Some(index) = class.body.iter().position(member_runs_static_code) else {
            return;
        };
        let ClassMember::StaticBlock(block) = &class.body[index] else {
            return;
        };
        let [Stmt::Expr(stmt)] = block.body.stmts.as_slice() else {
            return;
        };
        let names_this =
            wrapper_parts(self.helpers, strip_parens(&stmt.expr)).is_some_and(|(value, string)| {
                matches!(strip_parens(value), Expr::This(_)) && string == name
            });
        if names_this {
            class.body.remove(index);
            self.changed = true;
        }
    }
}

fn strip_parens_mut(expr: &mut Expr) -> &mut Expr {
    match expr {
        Expr::Paren(paren) => strip_parens_mut(&mut paren.expr),
        _ => expr,
    }
}

/// Whether the class's `name` is the one the language gives it: no static
/// member may be called `name` (a computed key could be), and no decorator
/// may run.
fn class_takes_plain_name(class: &Class) -> bool {
    if !class.decorators.is_empty() {
        return false;
    }
    let could_be_name = |key: &PropName| match key {
        PropName::Ident(key) => key.sym == "name",
        PropName::Str(key) => key.value == *"name",
        PropName::Computed(_) => true,
        PropName::Num(_) | PropName::BigInt(_) => false,
    };
    class.body.iter().all(|member| match member {
        ClassMember::Method(method) => {
            method.function.decorators.is_empty()
                && !(method.is_static && could_be_name(&method.key))
        }
        ClassMember::PrivateMethod(method) => method.function.decorators.is_empty(),
        ClassMember::ClassProp(prop) => {
            prop.decorators.is_empty() && !(prop.is_static && could_be_name(&prop.key))
        }
        ClassMember::PrivateProp(prop) => prop.decorators.is_empty(),
        ClassMember::AutoAccessor(accessor) => {
            accessor.decorators.is_empty()
                && !(accessor.is_static
                    && match &accessor.key {
                        Key::Public(key) => could_be_name(key),
                        Key::Private(_) => false,
                    })
        }
        ClassMember::Constructor(ctor) => ctor.params.iter().all(|param| match param {
            ParamOrTsParamProp::Param(param) => param.decorators.is_empty(),
            ParamOrTsParamProp::TsParamProp(prop) => prop.decorators.is_empty(),
        }),
        ClassMember::TsIndexSignature(_) | ClassMember::Empty(_) | ClassMember::StaticBlock(_) => {
            true
        }
    })
}

/// A static block or a static field initializer: code that runs while the
/// class is being defined, with the class in reach.
fn member_runs_static_code(member: &ClassMember) -> bool {
    match member {
        ClassMember::StaticBlock(_) => true,
        ClassMember::ClassProp(prop) => prop.is_static && prop.value.is_some(),
        ClassMember::PrivateProp(prop) => prop.is_static && prop.value.is_some(),
        ClassMember::AutoAccessor(accessor) => accessor.is_static && accessor.value.is_some(),
        _ => false,
    }
}

fn prop_name_str(key: &PropName) -> Option<&str> {
    match key {
        PropName::Ident(key) => Some(key.sym.as_ref()),
        PropName::Str(key) => key.value.as_str(),
        _ => None,
    }
}

/// One statement of a list, for [`drop_declaration_wrappers`].
enum ListItem<'a> {
    /// `helper(F, "F");`
    Wrapper(BindingKey),
    /// A function declaration. It is hoisted, so its body can run before
    /// any statement of the list.
    Function(BindingKey),
    Class(BindingKey, &'a Class),
    Other,
}

fn stmt_item<'s>(helpers: &HashSet<BindingKey>, stmt: &'s Stmt) -> ListItem<'s> {
    match stmt {
        Stmt::Expr(expr) => match wrapper_parts(helpers, strip_parens(&expr.expr)) {
            Some((value, name)) => match strip_parens(value) {
                Expr::Ident(ident) if ident.sym == *name => ListItem::Wrapper(binding_key(ident)),
                _ => ListItem::Other,
            },
            None => ListItem::Other,
        },
        Stmt::Decl(decl) => decl_item(decl),
        _ => ListItem::Other,
    }
}

fn module_item<'s>(helpers: &HashSet<BindingKey>, item: &'s ModuleItem) -> ListItem<'s> {
    match item {
        ModuleItem::Stmt(stmt) => stmt_item(helpers, stmt),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => decl_item(&export.decl),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(export)) => match &export.decl {
            DefaultDecl::Fn(FnExpr {
                ident: Some(ident),
                function,
            }) if function.body.is_some() => ListItem::Function(binding_key(ident)),
            DefaultDecl::Class(ClassExpr {
                ident: Some(ident),
                class,
            }) => ListItem::Class(binding_key(ident), class),
            _ => ListItem::Other,
        },
        ModuleItem::ModuleDecl(_) => ListItem::Other,
    }
}

fn decl_item(decl: &Decl) -> ListItem<'_> {
    match decl {
        Decl::Fn(FnDecl {
            ident,
            declare: false,
            function,
        }) if function.body.is_some() => ListItem::Function(binding_key(ident)),
        Decl::Class(ClassDecl {
            ident,
            declare: false,
            class,
        }) => ListItem::Class(binding_key(ident), class),
        _ => ListItem::Other,
    }
}

/// Drops `helper(F, "F")` statements that follow `function F` or `class F`
/// in the same list. Returns whether any went.
///
/// `F` gets its own `name` when it is declared, so the wrapper changes
/// nothing unless code that runs before it rewrites `name` (`delete F.name`,
/// `Object.defineProperty(F, …)`). That code needs `F` as a value: calling
/// it, `new`, a member read or write (`name` is read-only either way) or
/// `typeof` cannot. Code that may run first is every statement before the
/// wrapper and every hoisted function declaration of the list, so `F` may
/// not escape in any of them. A class runs its static code when declared,
/// so it may have none. Not modeled: a module that imports `F` in an import
/// cycle and runs first, and `arguments.callee`.
fn drop_declaration_wrappers<T>(
    list: &mut Vec<T>,
    uses: &BindingUseIndex,
    view: impl Fn(&T) -> ListItem<'_>,
) -> bool
where
    T: for<'v> VisitWith<EscapingRefs<'v>>,
{
    let mut targets = HashSet::default();
    for item in list.iter() {
        if let ListItem::Wrapper(key) = view(item) {
            targets.insert(key);
        }
    }
    if targets.is_empty() {
        return false;
    }

    let mut declarations: HashMap<BindingKey, usize> = HashMap::default();
    let mut first_escape: HashMap<BindingKey, usize> = HashMap::default();
    let mut hoisted_escape: HashSet<BindingKey> = HashSet::default();
    for (index, item) in list.iter().enumerate() {
        let hoisted = match view(item) {
            ListItem::Wrapper(_) => continue,
            ListItem::Function(key) => {
                if targets.contains(&key) {
                    declarations.insert(key, index);
                }
                true
            }
            ListItem::Class(key, class) => {
                if targets.contains(&key)
                    && class_takes_plain_name(class)
                    && !class.body.iter().any(member_runs_static_code)
                {
                    declarations.insert(key, index);
                }
                false
            }
            ListItem::Other => false,
        };
        let mut finder = EscapingRefs {
            targets: &targets,
            found: HashSet::default(),
        };
        item.visit_with(&mut finder);
        for key in finder.found {
            first_escape.entry(key.clone()).or_insert(index);
            if hoisted {
                hoisted_escape.insert(key);
            }
        }
    }

    let mut removable = HashSet::default();
    for (index, item) in list.iter().enumerate() {
        let ListItem::Wrapper(key) = view(item) else {
            continue;
        };
        if declarations
            .get(&key)
            .is_some_and(|declared_at| *declared_at < index)
            && uses.has_single_declaration(&key)
            && !uses.has_direct_write(&key)
            && first_escape.get(&key).is_none_or(|escape| *escape > index)
            && !hoisted_escape.contains(&key)
        {
            removable.insert(index);
        }
    }
    if removable.is_empty() {
        return false;
    }
    let mut index = 0;
    list.retain(|_| {
        let keep = !removable.contains(&index);
        index += 1;
        keep
    });
    true
}

impl VisitMut for Unwrapper<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        self.try_unwrap(expr, None);
    }

    fn visit_mut_var_declarator(&mut self, declarator: &mut VarDeclarator) {
        declarator.visit_mut_children_with(self);
        if let Pat::Ident(binding) = &declarator.name {
            let name = binding.sym.clone();
            self.unwrap_named_value(declarator.init.as_deref_mut(), &name);
        }
    }

    fn visit_mut_assign_expr(&mut self, assign: &mut AssignExpr) {
        assign.visit_mut_children_with(self);
        let names = matches!(
            assign.op,
            AssignOp::Assign | AssignOp::AndAssign | AssignOp::OrAssign | AssignOp::NullishAssign
        );
        if let (true, AssignTarget::Simple(SimpleAssignTarget::Ident(target))) =
            (names, &assign.left)
        {
            let name = target.sym.clone();
            self.unwrap_named_value(Some(&mut assign.right), &name);
        }
    }

    fn visit_mut_key_value_prop(&mut self, prop: &mut KeyValueProp) {
        prop.visit_mut_children_with(self);
        // `__proto__: value` sets the prototype and names nothing.
        if let Some(name) = prop_name_str(&prop.key).filter(|name| *name != "__proto__") {
            let name = name.to_string();
            self.unwrap_named_value(Some(&mut prop.value), &name);
        }
    }

    fn visit_mut_class_prop(&mut self, prop: &mut ClassProp) {
        prop.visit_mut_children_with(self);
        if let Some(name) = prop_name_str(&prop.key) {
            let name = name.to_string();
            self.unwrap_named_value(prop.value.as_deref_mut(), &name);
        }
    }

    fn visit_mut_private_prop(&mut self, prop: &mut PrivateProp) {
        prop.visit_mut_children_with(self);
        let name = format!("#{}", prop.key.name);
        self.unwrap_named_value(prop.value.as_deref_mut(), &name);
    }

    fn visit_mut_assign_pat_prop(&mut self, prop: &mut AssignPatProp) {
        prop.visit_mut_children_with(self);
        let name = prop.key.sym.clone();
        self.unwrap_named_value(prop.value.as_deref_mut(), &name);
    }

    fn visit_mut_assign_pat(&mut self, pat: &mut AssignPat) {
        pat.visit_mut_children_with(self);
        if let Pat::Ident(binding) = &*pat.left {
            let name = binding.sym.clone();
            self.unwrap_named_value(Some(&mut pat.right), &name);
        }
    }

    fn visit_mut_export_default_expr(&mut self, export: &mut ExportDefaultExpr) {
        // The exported call itself stays: without it, `export default
        // function F() {}` would print as a declaration, which also binds
        // `F`. esbuild names a default export through a declared binding
        // (`var in_default = __name(…, "default")`) instead.
        export.expr.visit_mut_children_with(self);
    }

    fn visit_mut_export_default_decl(&mut self, export: &mut ExportDefaultDecl) {
        export.visit_mut_children_with(self);
        if let DefaultDecl::Class(ClassExpr { ident: None, class }) = &mut export.decl {
            self.drop_static_name_block(class, "default");
        }
    }

    fn visit_mut_class_decl(&mut self, decl: &mut ClassDecl) {
        decl.visit_mut_children_with(self);
        let name = decl.ident.sym.clone();
        self.drop_static_name_block(&mut decl.class, &name);
    }

    fn visit_mut_class_expr(&mut self, expr: &mut ClassExpr) {
        expr.visit_mut_children_with(self);
        if let Some(ident) = &expr.ident {
            let name = ident.sym.clone();
            self.drop_static_name_block(&mut expr.class, &name);
        }
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        let helpers = self.helpers;
        self.changed |=
            drop_declaration_wrappers(items, self.uses, |item| module_item(helpers, item));
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        let helpers = self.helpers;
        self.changed |=
            drop_declaration_wrappers(stmts, self.uses, |stmt| stmt_item(helpers, stmt));
    }
}

/// References to `targets` that hand the binding to other code, which could
/// then rewrite its `name`. A call, `new`, a tagged template, a member read
/// or write and `typeof` do not; `delete F.x` does. Declarations and
/// assignment targets are not references here (writes are checked
/// separately).
struct EscapingRefs<'a> {
    targets: &'a HashSet<BindingKey>,
    found: HashSet<BindingKey>,
}

impl EscapingRefs<'_> {
    fn is_target(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Ident(ident) if self.targets.contains(&binding_key(ident)))
    }
}

impl Visit for EscapingRefs<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        let key = binding_key(ident);
        if self.targets.contains(&key) {
            self.found.insert(key);
        }
    }

    fn visit_binding_ident(&mut self, _: &BindingIdent) {}

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        decl.function.visit_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        decl.class.visit_with(self);
    }

    fn visit_fn_expr(&mut self, expr: &FnExpr) {
        expr.function.visit_with(self);
    }

    fn visit_class_expr(&mut self, expr: &ClassExpr) {
        expr.class.visit_with(self);
    }

    fn visit_callee(&mut self, callee: &Callee) {
        match callee {
            Callee::Expr(expr) if self.is_target(expr) => {}
            _ => callee.visit_children_with(self),
        }
    }

    fn visit_new_expr(&mut self, new_expr: &NewExpr) {
        if !self.is_target(&new_expr.callee) {
            new_expr.callee.visit_with(self);
        }
        new_expr.args.visit_with(self);
    }

    fn visit_tagged_tpl(&mut self, tagged: &TaggedTpl) {
        if !self.is_target(&tagged.tag) {
            tagged.tag.visit_with(self);
        }
        tagged.tpl.visit_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if self.is_target(&member.obj) {
            member.prop.visit_with(self);
        } else {
            member.visit_children_with(self);
        }
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        match (unary.op, &*unary.arg) {
            (UnaryOp::Delete, Expr::Member(member)) if self.is_target(&member.obj) => {
                if let Expr::Ident(ident) = &*member.obj {
                    self.found.insert(binding_key(ident));
                }
                member.prop.visit_with(self);
            }
            (UnaryOp::TypeOf, arg) if self.is_target(arg) => {}
            _ => unary.visit_children_with(self),
        }
    }
}
