use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BindingIdent, CallExpr, Callee, Class, ClassMember,
    ClassProp, Decl, Expr, ExprOrSpread, ExprStmt, Ident, IdentName, KeyValueProp, Lit, MemberExpr,
    MemberProp, Module, ModuleItem, Pat, PrivateName, PrivateProp, Prop, PropName, PropOrSpread,
    SimpleAssignTarget, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::helper_matcher::{binding_key, remove_unused_helper_declarations};
use super::lowered_private_field_helpers::{
    collect_esbuild_public_field_helpers, collect_lowered_private_helpers, LoweredPrivateHelper,
    LoweredPrivateHelpers, PrivateStorage, PublicFieldHelpers,
};
use super::transpiler_helper_utils::{
    tslib_member_ts_helper_kind, tslib_require_ts_helper_kind, BindingKey, LocalHelperContext,
    TranspilerHelperKind, TsHelperKind,
};
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
    private_maps: HashMap<BindingKey, Atom>,
    private_brand_sets: HashSet<BindingKey>,
    private_get_helpers: PrivateHelperCallees,
    private_set_helpers: PrivateHelperCallees,
    lowered_private_helpers: LoweredPrivateHelpers,
    private_single_owner_maps: HashSet<BindingKey>,
    consumed_private_maps: HashSet<BindingKey>,
}

// Keep raw TypeScript helper identity separate from field ownership proof.
struct PrivateHelperCallees {
    bindings: HashSet<BindingKey>,
    namespaces: HashSet<BindingKey>,
    kind: TsHelperKind,
    unresolved_mark: Mark,
}
impl PrivateHelperCallees {
    fn empty(kind: TsHelperKind, unresolved_mark: Mark) -> Self {
        Self {
            bindings: HashSet::default(),
            namespaces: HashSet::default(),
            kind,
            unresolved_mark,
        }
    }
    fn collect(
        module: &Module,
        helpers: &LocalHelperContext,
        kind: TsHelperKind,
        unresolved_mark: Mark,
    ) -> Self {
        let mut written =
            crate::analysis::binding_uses::BindingUseIndex::collect_direct_write_bindings(module);
        struct Hazards<'a> {
            written: &'a mut HashSet<BindingKey>,
        }
        impl Visit for Hazards<'_> {
            fn visit_assign_expr(&mut self, assign: &AssignExpr) {
                if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left {
                    if let Expr::Ident(id) = member.obj.as_ref() {
                        self.written.insert(binding_key(id));
                    }
                }
                assign.visit_children_with(self);
            }
            fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
                if let Expr::Member(member) = update.arg.as_ref() {
                    if let Expr::Ident(id) = member.obj.as_ref() {
                        self.written.insert(binding_key(id));
                    }
                }
                update.visit_children_with(self);
            }
        }
        let mut hazards = Hazards {
            written: &mut written,
        };
        module.visit_with(&mut hazards);
        Self {
            bindings: helpers
                .ts_helpers_of_kind(kind)
                .into_iter()
                .filter(|key| !written.contains(key))
                .collect(),
            namespaces: helpers
                .tslib_namespaces()
                .iter()
                .filter(|key| !written.contains(*key))
                .cloned()
                .collect(),
            kind,
            unresolved_mark,
        }
    }
    fn iter(&self) -> impl Iterator<Item = &BindingKey> {
        self.bindings.iter()
    }
    fn matches(&self, expr: &Expr) -> bool {
        let expr = crate::utils::paren::strip_parens(expr);
        if let Expr::Ident(id) = expr {
            return self.bindings.contains(&binding_key(id));
        }
        tslib_member_ts_helper_kind(expr, &self.namespaces)
            .or_else(|| tslib_require_ts_helper_kind(expr, Some(self.unresolved_mark)))
            == Some(self.kind)
    }
}

impl UnClassFields {
    pub fn new(level: RewriteLevel) -> Self {
        Self::new_with_mark(Mark::new(), level)
    }

    pub fn new_with_mark(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            level,
            unresolved_mark,
            define_property_helpers: HashSet::default(),
            private_maps: HashMap::default(),
            private_brand_sets: HashSet::default(),
            private_get_helpers: PrivateHelperCallees::empty(
                TsHelperKind::ClassPrivateFieldGet,
                unresolved_mark,
            ),
            private_set_helpers: PrivateHelperCallees::empty(
                TsHelperKind::ClassPrivateFieldSet,
                unresolved_mark,
            ),
            lowered_private_helpers: LoweredPrivateHelpers::default(),
            private_single_owner_maps: HashSet::default(),
            consumed_private_maps: HashSet::default(),
        }
    }

    pub(crate) fn run_with_helpers(
        &mut self,
        module: &mut Module,
        local_helpers: &LocalHelperContext,
    ) {
        // Field recovery drops `__init` methods, private WeakMap declarations,
        // and helper bindings; a `with` statement or direct eval anywhere can
        // still reach those names, so the module is left as is
        // (docs/rewrite-assumptions.md, dynamic-scope skip).
        if super::eval_utils::has_dynamic_scope_construct(module) {
            return;
        }
        let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::DefineProperty);
        // esbuild's `__publicField` is promoted like Babel's `_defineProperty`;
        // its helpers are removed here once no call is left.
        let public_field_helpers = if self.level >= RewriteLevel::Standard {
            collect_esbuild_public_field_helpers(module, self.unresolved_mark)
        } else {
            PublicFieldHelpers::default()
        };
        let public_field_helpers_used = !public_field_helpers.helpers.is_empty()
            && !super::transpiler_helper_utils::helpers_with_remaining_refs(
                module,
                &public_field_helpers
                    .helpers
                    .iter()
                    .map(|key| (key.clone(), TranspilerHelperKind::DefineProperty))
                    .collect(),
            )
            .is_empty();
        let previous_helpers = std::mem::replace(
            &mut self.define_property_helpers,
            helpers
                .keys()
                .chain(public_field_helpers.helpers.iter())
                .cloned()
                .collect(),
        );
        let private_maps = collect_private_weak_maps(module, self.unresolved_mark);
        let previous_private_maps = std::mem::replace(&mut self.private_maps, private_maps);
        let previous_private_brand_sets = std::mem::replace(
            &mut self.private_brand_sets,
            collect_private_brand_sets(module, self.unresolved_mark),
        );
        let previous_private_get_helpers = std::mem::replace(
            &mut self.private_get_helpers,
            PrivateHelperCallees::collect(
                module,
                local_helpers,
                TsHelperKind::ClassPrivateFieldGet,
                self.unresolved_mark,
            ),
        );
        let previous_private_set_helpers = std::mem::replace(
            &mut self.private_set_helpers,
            PrivateHelperCallees::collect(
                module,
                local_helpers,
                TsHelperKind::ClassPrivateFieldSet,
                self.unresolved_mark,
            ),
        );
        // Without a WeakMap or a WeakSet there is no backing map or brand for
        // a helper to reach.
        let lowered_private_helpers =
            if self.private_maps.is_empty() && self.private_brand_sets.is_empty() {
                LoweredPrivateHelpers::default()
            } else {
                collect_lowered_private_helpers(module, self.unresolved_mark)
            };
        let previous_lowered_private_helpers =
            std::mem::replace(&mut self.lowered_private_helpers, lowered_private_helpers);
        // Private methods first: a lowered method reads the class's maps
        // outside the class until it is moved back in.
        let recovered_private_methods =
            self.level >= RewriteLevel::Standard && !self.private_brand_sets.is_empty() && {
                let sites = PrivateCallSites {
                    tsc_get: &self.private_get_helpers,
                    tsc_set: &self.private_set_helpers,
                    lowered: &self.lowered_private_helpers.roles,
                    brand_sets: &self.private_brand_sets,
                    unresolved_mark: self.unresolved_mark,
                };
                recover_private_methods(module, &sites)
            };
        let previous_private_single_owner_maps = std::mem::replace(
            &mut self.private_single_owner_maps,
            collect_single_owner_private_maps(module, &self.private_maps, self.unresolved_mark),
        );
        let previous_consumed_private_maps = std::mem::take(&mut self.consumed_private_maps);

        let referenced_helpers =
            super::transpiler_helper_utils::helpers_with_remaining_refs(module, &helpers);

        module.visit_mut_children_with(self);

        if !self.consumed_private_maps.is_empty() {
            let removable_private_maps: HashSet<BindingKey> = self
                .consumed_private_maps
                .iter()
                .filter(|key| !private_map_has_remaining_refs(module, key, self.unresolved_mark))
                .cloned()
                .collect();
            if !removable_private_maps.is_empty() {
                remove_private_map_initializers(
                    module,
                    &removable_private_maps,
                    self.unresolved_mark,
                );
            }
        }

        let mut private_helpers: HashSet<BindingKey> = self
            .private_get_helpers
            .iter()
            .chain(self.private_set_helpers.iter())
            .cloned()
            .collect();
        if !self.consumed_private_maps.is_empty() || recovered_private_methods {
            private_helpers.extend(self.lowered_private_helpers.bindings.iter().cloned());
        }
        if public_field_helpers_used {
            private_helpers.extend(public_field_helpers.bindings.iter().cloned());
        }
        remove_unused_helper_declarations(module, &private_helpers);

        if !helpers.is_empty() {
            let consumed_helpers = helpers
                .into_iter()
                .filter(|(key, _)| referenced_helpers.contains(key))
                .collect::<HashMap<_, _>>();
            if !consumed_helpers.is_empty() {
                local_helpers.remove_helpers_with_dependencies(module, consumed_helpers);
            }
        }
        self.define_property_helpers = previous_helpers;
        self.private_maps = previous_private_maps;
        self.private_brand_sets = previous_private_brand_sets;
        self.private_get_helpers = previous_private_get_helpers;
        self.private_set_helpers = previous_private_set_helpers;
        self.lowered_private_helpers = previous_lowered_private_helpers;
        self.private_single_owner_maps = previous_private_single_owner_maps;
        self.consumed_private_maps = previous_consumed_private_maps;
    }
}

impl Default for UnClassFields {
    fn default() -> Self {
        Self::new(RewriteLevel::Standard)
    }
}

impl VisitMut for UnClassFields {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect_with_mark(module, self.unresolved_mark);
        self.run_with_helpers(module, &local_helpers);
    }

    fn visit_mut_class(&mut self, class: &mut Class) {
        class.visit_mut_children_with(self);

        // Collect __init* method bodies
        let mut init_bodies: HashMap<Atom, Vec<Stmt>> = HashMap::default();
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
        let mut inlined_names: crate::collections::HashSet<Atom> =
            crate::collections::HashSet::default();

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

        if inlined_names.is_empty() && self.level >= RewriteLevel::Standard {
            let sites = PrivateCallSites {
                tsc_get: &self.private_get_helpers,
                tsc_set: &self.private_set_helpers,
                lowered: &self.lowered_private_helpers.roles,
                brand_sets: &self.private_brand_sets,
                unresolved_mark: self.unresolved_mark,
            };
            let promoted_private_maps = promote_private_field_initializers(
                class,
                &self.private_maps,
                &self.private_single_owner_maps,
                &sites,
                &self.define_property_helpers,
            );
            if !promoted_private_maps.is_empty() {
                class.visit_mut_with(&mut PrivateFieldAccessRewriter {
                    promoted_maps: &promoted_private_maps,
                    sites: &sites,
                });
                self.consumed_private_maps
                    .extend(promoted_private_maps.into_keys());
            }
            if class.super_class.is_none() {
                promote_constructor_field_assignments(
                    class,
                    &self.define_property_helpers,
                    self.unresolved_mark,
                );
            }
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

/// Every `x = new WeakMap()` and `var x = new WeakMap()` binding, at any
/// depth, with its backing name. Which of them back a recoverable field is
/// decided by the ownership and lifetime proofs.
fn collect_private_weak_maps(module: &Module, unresolved_mark: Mark) -> HashMap<BindingKey, Atom> {
    struct Collector {
        unresolved_mark: Mark,
        maps: HashMap<BindingKey, Atom>,
    }
    impl Visit for Collector {
        fn visit_var_declarator(&mut self, decl: &swc_core::ecma::ast::VarDeclarator) {
            if let Pat::Ident(BindingIdent { id, .. }) = &decl.name {
                if decl
                    .init
                    .as_deref()
                    .is_some_and(|init| is_new_weak_map_expression(init, self.unresolved_mark))
                {
                    self.maps.insert(binding_key(id), id.sym.clone());
                }
            }
            decl.visit_children_with(self);
        }
        fn visit_expr(&mut self, expr: &Expr) {
            if let Expr::Assign(assign) = expr {
                if let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left {
                    if private_weak_map_assignment_key(expr, self.unresolved_mark).is_some() {
                        self.maps.insert(binding_key(&left.id), left.id.sym.clone());
                    }
                }
            }
            expr.visit_children_with(self);
        }
    }
    let mut collector = Collector {
        unresolved_mark,
        maps: HashMap::default(),
    };
    module.visit_with(&mut collector);
    collector.maps
}

/// Every binding created as `new WeakSet()`, at any depth.
fn collect_private_brand_sets(module: &Module, unresolved_mark: Mark) -> HashSet<BindingKey> {
    struct Collector {
        unresolved_mark: Mark,
        sets: HashSet<BindingKey>,
    }
    impl Collector {
        fn is_new_weak_set(&self, expr: &Expr) -> bool {
            matches!(expr, Expr::New(new_expr)
                if new_expr.args.as_ref().is_none_or(|args| args.is_empty())
                    && matches!(new_expr.callee.as_ref(), Expr::Ident(id)
                        if id.sym.as_ref() == "WeakSet" && id.ctxt.outer() == self.unresolved_mark))
        }
    }
    impl Visit for Collector {
        fn visit_var_declarator(&mut self, decl: &swc_core::ecma::ast::VarDeclarator) {
            if let Pat::Ident(BindingIdent { id, .. }) = &decl.name {
                if decl
                    .init
                    .as_deref()
                    .is_some_and(|init| self.is_new_weak_set(init))
                {
                    self.sets.insert(binding_key(id));
                }
            }
            decl.visit_children_with(self);
        }
        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left {
                if assign.op == AssignOp::Assign && self.is_new_weak_set(&assign.right) {
                    self.sets.insert(binding_key(&left.id));
                }
            }
            assign.visit_children_with(self);
        }
    }
    let mut collector = Collector {
        unresolved_mark,
        sets: HashSet::default(),
    };
    module.visit_with(&mut collector);
    collector.sets
}

fn collect_single_owner_private_maps(
    module: &Module,
    private_maps: &HashMap<BindingKey, Atom>,
    unresolved_mark: Mark,
) -> HashSet<BindingKey> {
    if private_maps.is_empty() {
        return HashSet::default();
    }

    let mut class_counts: HashMap<BindingKey, usize> = HashMap::default();
    let mut counter = PrivateMapClassConsumerCounter {
        private_maps,
        class_counts: &mut class_counts,
    };
    module.visit_with(&mut counter);

    let mut outside_finder = OutsidePrivateMapRefFinder {
        unresolved_mark,
        private_maps,
        refs: HashSet::default(),
    };
    module.visit_with(&mut outside_finder);

    let candidates: HashSet<BindingKey> = private_maps
        .keys()
        .filter(|key| class_counts.get(*key).copied() == Some(1))
        .filter(|key| !outside_finder.refs.contains(*key))
        .filter(|key| private_map_initializer_count(module, key, unresolved_mark) == 1)
        .cloned()
        .collect();
    if candidates.is_empty() {
        return HashSet::default();
    }
    let mut prover = PrivateMapLifetimeProver {
        candidates: &candidates,
        unresolved_mark,
        proven: HashSet::default(),
    };
    module.visit_with(&mut prover);
    prover.proven
}

// Count initialization writes throughout the AST, including comma
// expressions and nested functions. The statement-list proof alone would
// miss a later reset hidden inside a sequence.
fn private_map_initializer_count(
    module: &Module,
    key: &BindingKey,
    unresolved_mark: Mark,
) -> usize {
    struct InitializerCounter<'a> {
        key: &'a BindingKey,
        unresolved_mark: Mark,
        count: usize,
    }
    impl Visit for InitializerCounter<'_> {
        fn visit_var_declarator(&mut self, decl: &swc_core::ecma::ast::VarDeclarator) {
            if matches!(&decl.name, Pat::Ident(binding) if binding_key(&binding.id) == *self.key)
                && decl
                    .init
                    .as_deref()
                    .is_some_and(|expr| is_new_weak_map_expression(expr, self.unresolved_mark))
            {
                self.count += 1;
            }
            decl.visit_children_with(self);
        }
        fn visit_expr(&mut self, expr: &Expr) {
            if private_weak_map_assignment_key(expr, self.unresolved_mark).as_ref()
                == Some(self.key)
            {
                self.count += 1;
            }
            expr.visit_children_with(self);
        }
    }
    let mut counter = InitializerCounter {
        key,
        unresolved_mark,
        count: 0,
    };
    module.visit_with(&mut counter);
    counter.count
}

/// An item of a statement list a backing map can be created in: the module
/// body, or a function body.
#[derive(Clone, Copy)]
enum ListItem<'a> {
    Stmt(&'a Stmt),
    ModuleDecl(&'a swc_core::ecma::ast::ModuleDecl),
}

/// Proves, for each candidate map, the lifetime of its single initializer
/// in the statement list that holds it (see `list_gives_stable_lifetime`).
struct PrivateMapLifetimeProver<'a> {
    candidates: &'a HashSet<BindingKey>,
    unresolved_mark: Mark,
    proven: HashSet<BindingKey>,
}

impl PrivateMapLifetimeProver<'_> {
    fn check_list(&mut self, items: &[ListItem], is_function_body: bool) {
        let is_new_weak_map = |expr: &Expr| is_new_weak_map_expression(expr, self.unresolved_mark);
        for key in self.candidates {
            let Some(initializer) = items
                .iter()
                .position(|item| list_item_initializes(item, key, &is_new_weak_map))
            else {
                continue;
            };
            if list_gives_stable_lifetime(
                items,
                initializer,
                key,
                is_function_body,
                self.unresolved_mark,
            ) {
                self.proven.insert(key.clone());
            }
        }
    }

    fn check_stmts(&mut self, stmts: &[Stmt]) {
        let items: Vec<ListItem> = stmts.iter().map(ListItem::Stmt).collect();
        self.check_list(&items, true);
    }
}

impl Visit for PrivateMapLifetimeProver<'_> {
    fn visit_module(&mut self, module: &Module) {
        let items: Vec<ListItem> = module
            .body
            .iter()
            .map(|item| match item {
                ModuleItem::Stmt(stmt) => ListItem::Stmt(stmt),
                ModuleItem::ModuleDecl(decl) => ListItem::ModuleDecl(decl),
            })
            .collect();
        self.check_list(&items, false);
        module.visit_children_with(self);
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        if let Some(body) = &function.body {
            self.check_stmts(&body.stmts);
        }
        function.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        if let swc_core::ecma::ast::ArrowFunctionBody::FunctionBody(body) = arrow.body.as_ref() {
            self.check_stmts(&body.stmts);
        }
        arrow.visit_children_with(self);
    }
}

/// `var key = init` (the only declarator) or `key = init;`, where `is_init`
/// accepts `init`.
fn list_item_initializes(
    item: &ListItem,
    key: &BindingKey,
    is_init: &dyn Fn(&Expr) -> bool,
) -> bool {
    match item {
        ListItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
            var.decls.len() == 1
                && var.decls.iter().any(|decl| {
                    matches!(&decl.name, Pat::Ident(binding) if binding_key(&binding.id) == *key)
                        && decl.init.as_deref().is_some_and(is_init)
                })
        }
        ListItem::Stmt(Stmt::Expr(expr)) => matches!(expr.expr.as_ref(), Expr::Assign(assign)
            if assign.op == AssignOp::Assign
                && matches!(&assign.left, AssignTarget::Simple(SimpleAssignTarget::Ident(left))
                    if binding_key(&left.id) == *key)
                && is_init(&assign.right)),
        _ => false,
    }
}

// A backing map must exist before instances can be constructed, and must
// never be reset. The usual tsc and esbuild form initializes it immediately
// after the class; only a class without definition-time executable members
// is safe there. In a function body the map must also be that body's own
// binding: one declared outside would be reset by each call while instances
// of the previous call's class still use it.
fn list_gives_stable_lifetime(
    items: &[ListItem],
    initializer: usize,
    key: &BindingKey,
    is_function_body: bool,
    unresolved_mark: Mark,
) -> bool {
    if is_function_body && !list_declares_var(items, key) {
        return false;
    }
    let references_key = |class: &Class| class_body_references(class, key);
    let mut owner = None;
    for (index, item) in items.iter().enumerate() {
        if let Some((class, binding)) = private_map_class_owner(item, unresolved_mark) {
            if references_key(class) {
                owner = Some((index, Some((class, binding))));
            }
        } else if index > initializer && item_evaluates_owner_class(item, &references_key) {
            owner = Some((index, None));
        }
    }
    let Some((owner, owner_form)) = owner else {
        return false;
    };
    if initializer < owner {
        return true;
    }
    // Only a class in one of the owner forms can come before its map.
    let Some((class, binding)) = owner_form else {
        return false;
    };
    initialized_right_after_owner(items, owner, class, binding, initializer, unresolved_mark)
}

fn list_declares_var(items: &[ListItem], key: &BindingKey) -> bool {
    items.iter().any(|item| {
        matches!(item, ListItem::Stmt(Stmt::Decl(Decl::Var(var)))
            if var.decls.iter().any(|decl| matches!(&decl.name,
                Pat::Ident(binding) if binding_key(&binding.id) == *key)))
    })
}

/// Whether nothing between the owner class and the initializer after it can
/// construct an instance, and the class runs no code when defined.
fn initialized_right_after_owner(
    items: &[ListItem],
    owner: usize,
    class: &Class,
    binding: Option<&Ident>,
    initializer: usize,
    unresolved_mark: Mark,
) -> bool {
    // Local export lists and function declarations have no evaluation step.
    // Reading the already-created owner binding for a default export cannot
    // run user code either. Do not allow arbitrary identifiers: an
    // unresolved global can invoke a getter.
    items[owner + 1..initializer].iter().all(|item| match item {
        ListItem::ModuleDecl(swc_core::ecma::ast::ModuleDecl::ExportNamed(export)) => {
            export.src.is_none()
        }
        ListItem::ModuleDecl(swc_core::ecma::ast::ModuleDecl::ExportDefaultExpr(export)) => {
            matches!(crate::utils::paren::strip_parens(&export.expr), Expr::Ident(id)
                if binding.is_some_and(|owner_id| binding_key(owner_id) == binding_key(id)))
        }
        ListItem::Stmt(Stmt::Empty(_) | Stmt::Decl(Decl::Fn(_))) => true,
        // esbuild creates each of a class's maps and sets, and assigns its
        // lowered private methods, in turn after it.
        ListItem::Stmt(Stmt::Expr(expr)) => {
            is_local_brand_collection_initializer(&expr.expr, unresolved_mark)
        }
        _ => false,
    }) && class.decorators.is_empty()
        && class.body.iter().all(|member| match member {
            ClassMember::Constructor(_) | ClassMember::Empty(_) => true,
            ClassMember::Method(method) => {
                !matches!(method.key, PropName::Computed(_))
                    && method.function.decorators.is_empty()
            }
            // A private key is never computed.
            ClassMember::PrivateMethod(method) => method.function.decorators.is_empty(),
            _ => false,
        })
}

/// Whether the class body references `key`, outside nested classes.
fn class_body_references(class: &Class, key: &BindingKey) -> bool {
    struct Finder<'a> {
        key: &'a BindingKey,
        found: bool,
    }
    impl Visit for Finder<'_> {
        fn visit_class(&mut self, _class: &Class) {}
        fn visit_ident(&mut self, ident: &Ident) {
            if binding_key(ident) == *self.key {
                self.found = true;
            }
        }
    }
    let mut finder = Finder { key, found: false };
    class.body.visit_with(&mut finder);
    finder.found
}

/// Whether evaluating `item` once evaluates a class whose body references the
/// map, and only once: the class is reached through expressions alone, not
/// through a function, a loop, or another class. Minified code returns the
/// class from its factory directly, `return new class { … }`.
fn item_evaluates_owner_class(item: &ListItem, references_key: &dyn Fn(&Class) -> bool) -> bool {
    struct Finder<'a> {
        references_key: &'a dyn Fn(&Class) -> bool,
        found: bool,
    }
    impl Visit for Finder<'_> {
        fn visit_class(&mut self, class: &Class) {
            if (self.references_key)(class) {
                self.found = true;
            }
        }
        fn visit_function(&mut self, _: &swc_core::ecma::ast::Function) {}
        fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}
        fn visit_getter_prop(&mut self, _: &swc_core::ecma::ast::GetterProp) {}
        fn visit_setter_prop(&mut self, _: &swc_core::ecma::ast::SetterProp) {}
    }
    let mut finder = Finder {
        references_key,
        found: false,
    };
    match item {
        ListItem::Stmt(Stmt::Return(ret)) => ret.arg.visit_with(&mut finder),
        ListItem::Stmt(Stmt::Expr(expr)) => expr.expr.visit_with(&mut finder),
        ListItem::Stmt(Stmt::Decl(Decl::Var(var))) => var.visit_with(&mut finder),
        ListItem::ModuleDecl(swc_core::ecma::ast::ModuleDecl::ExportDefaultExpr(export)) => {
            export.expr.visit_with(&mut finder)
        }
        _ => {}
    }
    finder.found
}

// Identify the class and the outer binding that receives it. A named class
// expression's self binding is not the variable that owns the resulting class.
fn private_map_class_owner<'a>(
    item: &ListItem<'a>,
    unresolved_mark: Mark,
) -> Option<(&'a Class, Option<&'a Ident>)> {
    use crate::utils::paren::strip_parens;
    use swc_core::ecma::ast::{DefaultDecl, ModuleDecl};

    let decl = match *item {
        ListItem::Stmt(Stmt::Decl(decl)) => decl,
        ListItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => &export.decl,
        ListItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(export)) => {
            let DefaultDecl::Class(expr) = &export.decl else {
                return None;
            };
            return Some((&expr.class, expr.ident.as_ref()));
        }
        ListItem::Stmt(Stmt::Expr(stmt)) => {
            // Sequence splitting turns tsc class-expression factories into
            // `_a = class ...; map = new WeakMap(); const Foo = _a;`.
            let Expr::Assign(assign) = strip_parens(&stmt.expr) else {
                return None;
            };
            if assign.op != AssignOp::Assign {
                return None;
            }
            let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assign.left else {
                return None;
            };
            if binding.id.ctxt.outer() == unresolved_mark {
                return None;
            }
            let Expr::Class(expr) = strip_parens(&assign.right) else {
                return None;
            };
            return Some((&expr.class, Some(&binding.id)));
        }
        _ => return None,
    };
    match decl {
        Decl::Class(decl) => Some((&decl.class, Some(&decl.ident))),
        Decl::Var(var) if var.decls.len() == 1 => {
            let declarator = &var.decls[0];
            let Pat::Ident(binding) = &declarator.name else {
                return None;
            };
            let Expr::Class(expr) = strip_parens(declarator.init.as_deref()?) else {
                return None;
            };
            Some((&expr.class, Some(&binding.id)))
        }
        _ => None,
    }
}

struct PrivateMapClassConsumerCounter<'a> {
    private_maps: &'a HashMap<BindingKey, Atom>,
    class_counts: &'a mut HashMap<BindingKey, usize>,
}

impl Visit for PrivateMapClassConsumerCounter<'_> {
    fn visit_class(&mut self, class: &Class) {
        let mut collector = PrivateMapRefCollector {
            private_maps: self.private_maps,
            refs: HashSet::default(),
        };
        class.body.visit_with(&mut collector);
        for key in collector.refs {
            *self.class_counts.entry(key).or_insert(0) += 1;
        }
        class.visit_children_with(self);
    }
}

struct PrivateMapRefCollector<'a> {
    private_maps: &'a HashMap<BindingKey, Atom>,
    refs: HashSet<BindingKey>,
}

impl Visit for PrivateMapRefCollector<'_> {
    fn visit_class(&mut self, _class: &Class) {}

    fn visit_ident(&mut self, ident: &Ident) {
        let key = binding_key(ident);
        if self.private_maps.contains_key(&key) {
            self.refs.insert(key);
        }
    }
}

/// References to a map outside any class body, other than its declaration
/// and its `map = new WeakMap()` initializers.
struct OutsidePrivateMapRefFinder<'a> {
    unresolved_mark: Mark,
    private_maps: &'a HashMap<BindingKey, Atom>,
    refs: HashSet<BindingKey>,
}

impl Visit for OutsidePrivateMapRefFinder<'_> {
    fn visit_class(&mut self, _class: &Class) {}

    fn visit_var_declarator(&mut self, decl: &swc_core::ecma::ast::VarDeclarator) {
        if let Some(init) = &decl.init {
            init.visit_with(self);
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if private_weak_map_assignment_key(expr, self.unresolved_mark).is_some() {
            return;
        }
        expr.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        let key = binding_key(ident);
        if self.private_maps.contains_key(&key) {
            self.refs.insert(key);
        }
    }
}

fn private_name_from_tsc_backing_ident(sym: &Atom) -> Option<Atom> {
    let name = sym.as_ref().trim_start_matches('_');
    if name.is_empty() {
        return None;
    }
    let private_name = name.split_once('_').map_or(name, |(_, field)| field);
    if is_identifier_name(private_name) {
        Some(private_name.into())
    } else {
        None
    }
}

/// Babel, swc, and esbuild name the map `_x` for `#x` (`_x2` when the
/// name is taken).
fn private_name_from_underscore_backing_ident(sym: &Atom) -> Option<Atom> {
    let name = sym.as_ref();
    let name = name.strip_prefix('_').unwrap_or(name);
    (!name.is_empty() && is_identifier_name(name)).then(|| name.into())
}

fn is_new_weak_map_expression(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::New(new_expr) = expr else {
        return false;
    };
    if new_expr.args.as_ref().is_some_and(|args| !args.is_empty()) {
        return false;
    }
    matches!(new_expr.callee.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "WeakMap" && id.ctxt.outer() == unresolved_mark)
}

fn private_map_has_remaining_refs(
    module: &Module,
    key: &BindingKey,
    unresolved_mark: Mark,
) -> bool {
    struct RefFinder<'a> {
        key: &'a BindingKey,
        unresolved_mark: Mark,
        found: bool,
    }

    impl Visit for RefFinder<'_> {
        fn visit_var_declarator(&mut self, decl: &swc_core::ecma::ast::VarDeclarator) {
            if let Pat::Ident(BindingIdent { id, .. }) = &decl.name {
                if binding_key(id) == *self.key
                    && decl
                        .init
                        .as_deref()
                        .is_none_or(|init| is_new_weak_map_expression(init, self.unresolved_mark))
                {
                    return;
                }
            }
            decl.visit_children_with(self);
        }

        fn visit_expr(&mut self, expr: &Expr) {
            if private_weak_map_assignment_key(expr, self.unresolved_mark).as_ref()
                == Some(self.key)
            {
                return;
            }
            expr.visit_children_with(self);
        }

        fn visit_ident(&mut self, ident: &Ident) {
            if binding_key(ident) == *self.key {
                self.found = true;
            }
        }
    }

    let mut finder = RefFinder {
        key,
        unresolved_mark,
        found: false,
    };
    module.visit_with(&mut finder);
    finder.found
}

/// Drops the declarations and initializers of maps no code reads any more,
/// in the module body and in function bodies.
fn remove_private_map_initializers(
    module: &mut Module,
    removable: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) {
    struct Remover<'a> {
        removable: &'a HashSet<BindingKey>,
        unresolved_mark: Mark,
    }
    impl Remover<'_> {
        fn retain_stmt(&self, stmt: &mut Stmt) -> bool {
            match stmt {
                Stmt::Decl(Decl::Var(var_decl)) => {
                    var_decl.decls.retain(|decl| {
                        let Pat::Ident(BindingIdent { id, .. }) = &decl.name else {
                            return true;
                        };
                        if !self.removable.contains(&binding_key(id)) {
                            return true;
                        }
                        !decl.init.as_deref().is_none_or(|init| {
                            is_new_weak_map_expression(init, self.unresolved_mark)
                        })
                    });
                    !var_decl.decls.is_empty()
                }
                Stmt::Expr(expr_stmt) => remove_private_weak_map_assignment_expr(
                    &mut expr_stmt.expr,
                    self.removable,
                    self.unresolved_mark,
                ),
                _ => true,
            }
        }
    }
    impl VisitMut for Remover<'_> {
        fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
            items.retain_mut(|item| match item {
                ModuleItem::Stmt(stmt) => self.retain_stmt(stmt),
                ModuleItem::ModuleDecl(_) => true,
            });
            items.visit_mut_children_with(self);
        }

        fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
            stmts.retain_mut(|stmt| self.retain_stmt(stmt));
            stmts.visit_mut_children_with(self);
        }
    }
    module.visit_mut_with(&mut Remover {
        removable,
        unresolved_mark,
    });
}

fn remove_private_weak_map_assignment_expr(
    expr: &mut Box<Expr>,
    removable: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> bool {
    match expr.as_mut() {
        Expr::Seq(seq) => {
            seq.exprs.retain(|expr| {
                !private_weak_map_assignment_key(expr, unresolved_mark)
                    .is_some_and(|key| removable.contains(&key))
            });
            match seq.exprs.len() {
                0 => false,
                1 => {
                    *expr = seq.exprs.pop().expect("one sequence expr remains");
                    true
                }
                _ => true,
            }
        }
        _ => !private_weak_map_assignment_key(expr, unresolved_mark)
            .is_some_and(|key| removable.contains(&key)),
    }
}

/// `local = new WeakMap()`, `local = new WeakSet()`, or `local = function
/// () {…}`: no user code runs.
fn is_local_brand_collection_initializer(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Assign(assign) = expr else {
        return false;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left else {
        return false;
    };
    if assign.op != AssignOp::Assign || left.id.ctxt.outer() == unresolved_mark {
        return false;
    }
    match assign.right.as_ref() {
        Expr::Fn(_) => true,
        Expr::New(new_expr) => {
            new_expr.args.as_ref().is_none_or(|args| args.is_empty())
                && matches!(new_expr.callee.as_ref(), Expr::Ident(id)
                    if matches!(id.sym.as_ref(), "WeakMap" | "WeakSet") && id.ctxt.outer() == unresolved_mark)
        }
        _ => false,
    }
}

fn private_weak_map_assignment_key(expr: &Expr, unresolved_mark: Mark) -> Option<BindingKey> {
    let Expr::Assign(assign) = expr else {
        return None;
    };
    if assign.op != AssignOp::Assign || !is_new_weak_map_expression(&assign.right, unresolved_mark)
    {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left else {
        return None;
    };
    Some(binding_key(&left.id))
}

fn is_new_weak_set_expression(expr: &Expr, unresolved_mark: Mark) -> bool {
    matches!(expr, Expr::New(new_expr)
        if new_expr.args.as_ref().is_none_or(|args| args.is_empty())
            && matches!(new_expr.callee.as_ref(), Expr::Ident(id)
                if id.sym.as_ref() == "WeakSet" && id.ctxt.outer() == unresolved_mark))
}

fn is_function_expression(expr: &Expr) -> bool {
    matches!(expr, Expr::Fn(_))
}

/// How the module names a brand set or a lowered private method: its
/// definitions (`new WeakSet()` for a brand, a function declaration or
/// function expression for a method, as a declarator or an assignment), and
/// every other reference, exports included.
#[derive(Default, Clone, Copy)]
struct MethodBindingRefs {
    defs: usize,
    refs: usize,
}

fn count_method_binding_refs(
    module: &Module,
    brands: &HashSet<BindingKey>,
    methods: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> HashMap<BindingKey, MethodBindingRefs> {
    struct Counter<'a> {
        brands: &'a HashSet<BindingKey>,
        methods: &'a HashSet<BindingKey>,
        unresolved_mark: Mark,
        counts: HashMap<BindingKey, MethodBindingRefs>,
    }
    impl Counter<'_> {
        fn is_definition(&self, key: &BindingKey, expr: &Expr) -> bool {
            if self.brands.contains(key) {
                is_new_weak_set_expression(expr, self.unresolved_mark)
            } else {
                is_function_expression(expr)
            }
        }
        fn tracks(&self, key: &BindingKey) -> bool {
            self.brands.contains(key) || self.methods.contains(key)
        }
        fn entry(&mut self, key: BindingKey) -> &mut MethodBindingRefs {
            self.counts.entry(key).or_default()
        }
    }
    impl Visit for Counter<'_> {
        fn visit_export_decl(&mut self, export: &swc_core::ecma::ast::ExportDecl) {
            // An exported binding is observable without a local reference.
            let mut exported = Vec::new();
            match &export.decl {
                Decl::Fn(decl) => exported.push(binding_key(&decl.ident)),
                Decl::Var(var) => {
                    for decl in &var.decls {
                        if let Pat::Ident(binding) = &decl.name {
                            exported.push(binding_key(&binding.id));
                        }
                    }
                }
                _ => {}
            }
            for key in exported {
                if self.tracks(&key) {
                    self.entry(key).refs += 1;
                }
            }
            export.visit_children_with(self);
        }
        fn visit_var_declarator(&mut self, decl: &swc_core::ecma::ast::VarDeclarator) {
            let Pat::Ident(binding) = &decl.name else {
                decl.visit_children_with(self);
                return;
            };
            let key = binding_key(&binding.id);
            if !self.tracks(&key) {
                decl.visit_children_with(self);
                return;
            }
            if let Some(init) = decl.init.as_deref() {
                if self.is_definition(&key, init) {
                    self.entry(key).defs += 1;
                } else {
                    self.entry(key).refs += 1;
                }
                init.visit_with(self);
            }
        }
        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left {
                let key = binding_key(&left.id);
                if assign.op == AssignOp::Assign
                    && self.tracks(&key)
                    && self.is_definition(&key, &assign.right)
                {
                    self.entry(key).defs += 1;
                    assign.right.visit_with(self);
                    return;
                }
            }
            assign.visit_children_with(self);
        }
        fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
            let key = binding_key(&decl.ident);
            if self.methods.contains(&key) {
                self.entry(key).defs += 1;
                decl.function.visit_with(self);
                return;
            }
            decl.visit_children_with(self);
        }
        fn visit_ident(&mut self, ident: &Ident) {
            let key = binding_key(ident);
            if self.tracks(&key) {
                self.entry(key).refs += 1;
            }
        }
    }
    let mut counter = Counter {
        brands,
        methods,
        unresolved_mark,
        counts: HashMap::default(),
    };
    module.visit_with(&mut counter);
    counter.counts
}

/// Recovers private methods lowered to a brand set and a function outside
/// the class:
///
/// ```js
/// class C { constructor() { add(this, _brand); } m() { return check(this, _brand, f).call(this, 1); } }
/// _brand = new WeakSet(); f = function (x) { … };
/// ```
///
/// becomes `class C { #f(x) { … } m() { return this.#f(1); } }`. The brand
/// is added by the constructor's leading statements (after a derived
/// class's only top-level super call), so the method exists from the start of
/// construction as a class method does. Each brand and method binding has
/// one definition in the statement list that holds the class, with the
/// backing-map lifetime (a function declaration is hoisted), and every other
/// reference is that brand add or a `check(this, brand, f).call(this, …)`
/// call in the class body or in a method moving into it: never exported,
/// written, or read otherwise. The
/// method must be a `function` that does not name itself or, by name, the
/// class. Returns whether a method was recovered.
fn recover_private_methods(module: &mut Module, sites: &PrivateCallSites) -> bool {
    struct KeyFinder<'a, 'b> {
        sites: &'a PrivateCallSites<'b>,
        brands: HashSet<BindingKey>,
        methods: HashSet<BindingKey>,
    }
    impl Visit for KeyFinder<'_, '_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Some(PrivateOp::Method { brand, method, .. }) = self.sites.op(call) {
                if self.sites.brand_sets.contains(&binding_key(brand)) {
                    self.brands.insert(binding_key(brand));
                    self.methods.insert(binding_key(method));
                }
            }
            call.visit_children_with(self);
        }
    }
    let mut finder = KeyFinder {
        sites,
        brands: HashSet::default(),
        methods: HashSet::default(),
    };
    module.visit_with(&mut finder);
    if finder.methods.is_empty() {
        return false;
    }
    let counts = count_method_binding_refs(
        module,
        &finder.brands,
        &finder.methods,
        sites.unresolved_mark,
    );
    let written =
        crate::analysis::binding_uses::BindingUseIndex::collect_direct_write_bindings(module);
    let mut recoverer = PrivateMethodRecoverer {
        sites,
        counts: &counts,
        written: &written,
        recovered: false,
    };
    module.visit_mut_with(&mut recoverer);
    recoverer.recovered
}

struct PrivateMethodRecoverer<'a, 'b> {
    sites: &'a PrivateCallSites<'b>,
    counts: &'a HashMap<BindingKey, MethodBindingRefs>,
    written: &'a HashSet<BindingKey>,
    recovered: bool,
}

/// The private methods one class gets back.
struct PrivateMethodPlan {
    owner: usize,
    /// Constructor statements that add the brands.
    brand_adds: Vec<usize>,
    /// Brand and method bindings whose definitions the list drops.
    consumed: HashSet<BindingKey>,
    methods: Vec<(BindingKey, Atom, Box<swc_core::ecma::ast::Function>)>,
}

impl VisitMut for PrivateMethodRecoverer<'_, '_> {
    fn visit_mut_module(&mut self, module: &mut Module) {
        module.visit_mut_children_with(self);
        loop {
            let items: Vec<ListItem> = module
                .body
                .iter()
                .map(|item| match item {
                    ModuleItem::Stmt(stmt) => ListItem::Stmt(stmt),
                    ModuleItem::ModuleDecl(decl) => ListItem::ModuleDecl(decl),
                })
                .collect();
            let Some(plan) = self.plan(&items, false) else {
                return;
            };
            let class = match &mut module.body[plan.owner] {
                ModuleItem::Stmt(stmt) => owner_class_in_stmt_mut(stmt),
                ModuleItem::ModuleDecl(decl) => owner_class_in_module_decl_mut(decl),
            };
            let Some(class) = class else {
                return;
            };
            self.apply(class, &plan);
            module.body.retain_mut(|item| match item {
                ModuleItem::Stmt(stmt) => retain_unconsumed_definition(stmt, &plan.consumed),
                ModuleItem::ModuleDecl(_) => true,
            });
        }
    }

    fn visit_mut_function(&mut self, function: &mut swc_core::ecma::ast::Function) {
        function.visit_mut_children_with(self);
        if let Some(body) = &mut function.body {
            self.recover_in_stmts(&mut body.stmts);
        }
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut swc_core::ecma::ast::ArrowExpr) {
        arrow.visit_mut_children_with(self);
        if let swc_core::ecma::ast::ArrowFunctionBody::FunctionBody(body) = arrow.body.as_mut() {
            self.recover_in_stmts(&mut body.stmts);
        }
    }
}

impl PrivateMethodRecoverer<'_, '_> {
    fn recover_in_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        loop {
            let items: Vec<ListItem> = stmts.iter().map(ListItem::Stmt).collect();
            let Some(plan) = self.plan(&items, true) else {
                return;
            };
            let Some(class) = owner_class_in_stmt_mut(&mut stmts[plan.owner]) else {
                return;
            };
            self.apply(class, &plan);
            stmts.retain_mut(|stmt| retain_unconsumed_definition(stmt, &plan.consumed));
        }
    }

    /// The first class in the list whose private methods can be recovered.
    fn plan(&self, items: &[ListItem], is_function_body: bool) -> Option<PrivateMethodPlan> {
        items.iter().enumerate().find_map(|(owner, item)| {
            let (class, _) = private_map_class_owner(item, self.sites.unresolved_mark)?;
            self.plan_class(items, owner, class, is_function_body)
        })
    }

    fn plan_class(
        &self,
        items: &[ListItem],
        owner: usize,
        class: &Class,
        is_function_body: bool,
    ) -> Option<PrivateMethodPlan> {
        let mark = self.sites.unresolved_mark;
        let ctor = class.body.iter().find_map(|member| match member {
            ClassMember::Constructor(ctor) => Some(ctor),
            _ => None,
        })?;
        let body = ctor.body.as_ref()?;
        let start = if class.super_class.is_some() {
            single_top_level_super_call(&body.stmts)? + 1
        } else {
            0
        };
        // The brand adds lead the initializers, as methods precede fields.
        let mut brands: Vec<BindingKey> = Vec::new();
        let mut brand_adds = Vec::new();
        for (index, stmt) in body.stmts.iter().enumerate().skip(start) {
            let Some(brand) = self.sites.brand_add_key(stmt) else {
                break;
            };
            // A second add throws on construction.
            if brands.contains(&brand) {
                return None;
            }
            brands.push(brand);
            brand_adds.push(index);
        }
        if brands.is_empty() {
            return None;
        }
        let is_new_weak_set = |expr: &Expr| is_new_weak_set_expression(expr, mark);
        for brand in &brands {
            let counts = self.counts.get(brand).copied().unwrap_or_default();
            let definition = items
                .iter()
                .position(|item| list_item_initializes(item, brand, &is_new_weak_set))?;
            if counts.defs != 1
                || !self.defined_in_time(items, owner, definition, brand, is_function_body)
            {
                return None;
            }
        }

        // The class body's method calls, by method: its brand and count.
        struct SiteFinder<'a, 'b> {
            sites: &'a PrivateCallSites<'b>,
            calls: Vec<(BindingKey, BindingKey, bool)>,
        }
        impl Visit for SiteFinder<'_, '_> {
            fn visit_call_expr(&mut self, call: &CallExpr) {
                if let Some(PrivateOp::Method { brand, method, tsc }) =
                    self.sites.private_method_call(call)
                {
                    self.calls
                        .push((binding_key(brand), binding_key(method), tsc));
                    call.args[1..].visit_with(self);
                    return;
                }
                call.visit_children_with(self);
            }
        }
        let mut finder = SiteFinder {
            sites: self.sites,
            calls: Vec::new(),
        };
        class.body.visit_with(&mut finder);
        // The methods move into the class, and so do the calls in their
        // bodies: one method calling another, or itself.
        let mut functions: HashMap<BindingKey, Box<swc_core::ecma::ast::Function>> =
            HashMap::default();
        let mut scanned = 0;
        while scanned < finder.calls.len() {
            let (brand, method, _) = finder.calls[scanned].clone();
            scanned += 1;
            if !brands.contains(&brand) || functions.contains_key(&method) {
                continue;
            }
            let function = self.method_definition(items, owner, &method, is_function_body)?;
            function.visit_with(&mut finder);
            functions.insert(method, function);
        }

        let mut brand_refs: HashMap<BindingKey, usize> =
            brands.iter().map(|brand| (brand.clone(), 1)).collect();
        // Method, its brand, its call count, and whether tsc lowered it.
        let mut methods: Vec<(BindingKey, BindingKey, usize, bool)> = Vec::new();
        for (brand, method, tsc) in finder.calls {
            let Some(refs) = brand_refs.get_mut(&brand) else {
                continue;
            };
            *refs += 1;
            match methods.iter_mut().find(|(key, ..)| *key == method) {
                Some((_, seen_brand, calls, seen_tsc)) => {
                    if *seen_brand != brand || *seen_tsc != tsc {
                        return None;
                    }
                    *calls += 1;
                }
                None => methods.push((method, brand, 1, tsc)),
            }
        }
        for (brand, refs) in &brand_refs {
            if self.counts.get(brand).map_or(0, |counts| counts.refs) != *refs {
                return None;
            }
        }
        if methods.is_empty() {
            return None;
        }

        let class_name = owner_class_inner_name(&items[owner]);
        let mut taken_names = class_private_names(class);
        let mut consumed: HashSet<BindingKey> = brands.iter().cloned().collect();
        let mut recovered = Vec::new();
        for (method, _, calls, tsc) in methods {
            let counts = self.counts.get(&method).copied().unwrap_or_default();
            if counts.defs != 1 || counts.refs != calls {
                return None;
            }
            let function = functions.remove(&method)?;
            if let Some((name, is_decl)) = &class_name {
                // Inside the class body, the class's own name binding wins.
                let same_binding = *is_decl && !self.written.contains(&binding_key(name));
                if function_names_sym(&function, name, same_binding) {
                    return None;
                }
            }
            let private_name = if tsc {
                private_name_from_tsc_backing_ident(&method.0)
            } else {
                private_name_from_lowered_method_ident(&method.0)
            }?;
            if !taken_names.insert(private_name.clone()) {
                return None;
            }
            consumed.insert(method.clone());
            recovered.push((method, private_name, function));
        }
        Some(PrivateMethodPlan {
            owner,
            brand_adds,
            consumed,
            methods: recovered,
        })
    }

    /// The method's one definition in the list: a function declaration, or a
    /// `function` expression with the backing-map lifetime. A function
    /// expression's own name must not be referenced in its body.
    fn method_definition(
        &self,
        items: &[ListItem],
        owner: usize,
        method: &BindingKey,
        is_function_body: bool,
    ) -> Option<Box<swc_core::ecma::ast::Function>> {
        for item in items {
            if let ListItem::Stmt(Stmt::Decl(Decl::Fn(decl))) = item {
                if binding_key(&decl.ident) == *method {
                    return Some(decl.function.clone());
                }
            }
        }
        let definition = items
            .iter()
            .position(|item| list_item_initializes(item, method, &is_function_expression))?;
        if !self.defined_in_time(items, owner, definition, method, is_function_body) {
            return None;
        }
        let init = match items[definition] {
            ListItem::Stmt(Stmt::Decl(Decl::Var(var))) => var.decls[0].init.as_deref()?,
            ListItem::Stmt(Stmt::Expr(expr)) => match expr.expr.as_ref() {
                Expr::Assign(assign) => &assign.right,
                _ => return None,
            },
            _ => return None,
        };
        let Expr::Fn(fn_expr) = init else {
            return None;
        };
        if let Some(self_name) = &fn_expr.ident {
            let key = binding_key(self_name);
            let mut finder = BindingRefFinder {
                key: &key,
                found: false,
            };
            fn_expr.function.visit_with(&mut finder);
            if finder.found {
                return None;
            }
        }
        Some(fn_expr.function.clone())
    }

    /// The backing-map lifetime, for the class at `owner`: the binding is
    /// defined once before the class, or right after it with nothing between
    /// that could construct an instance. In a function body the binding must
    /// be that body's own.
    fn defined_in_time(
        &self,
        items: &[ListItem],
        owner: usize,
        definition: usize,
        key: &BindingKey,
        is_function_body: bool,
    ) -> bool {
        if is_function_body && !list_declares_var(items, key) {
            return false;
        }
        if definition < owner {
            return true;
        }
        let mark = self.sites.unresolved_mark;
        let Some((class, binding)) = private_map_class_owner(&items[owner], mark) else {
            return false;
        };
        initialized_right_after_owner(items, owner, class, binding, definition, mark)
    }

    fn apply(&mut self, class: &mut Class, plan: &PrivateMethodPlan) {
        let names: HashMap<BindingKey, Atom> = plan
            .methods
            .iter()
            .map(|(key, name, _)| (key.clone(), name.clone()))
            .collect();
        let is_derived = class.super_class.is_some();
        let Some(ctor_index) = class
            .body
            .iter()
            .position(|member| matches!(member, ClassMember::Constructor(_)))
        else {
            return;
        };
        let mut remove_ctor = false;
        if let ClassMember::Constructor(ctor) = &mut class.body[ctor_index] {
            if let Some(body) = &mut ctor.body {
                for index in plan.brand_adds.iter().rev() {
                    body.stmts.remove(*index);
                }
                remove_ctor = !is_derived && body.stmts.is_empty() && ctor.params.is_empty();
            }
        }
        let methods = plan.methods.iter().map(|(_, name, function)| {
            ClassMember::PrivateMethod(swc_core::ecma::ast::PrivateMethod {
                span: DUMMY_SP,
                key: PrivateName {
                    span: DUMMY_SP,
                    name: name.clone(),
                },
                function: function.clone(),
                kind: swc_core::ecma::ast::MethodKind::Method,
                is_static: false,
                accessibility: None,
                is_abstract: false,
                is_optional: false,
                is_override: false,
            })
        });
        let insert_at = if remove_ctor {
            class.body.remove(ctor_index);
            ctor_index
        } else {
            ctor_index + 1
        };
        class.body.splice(insert_at..insert_at, methods);
        // The recovered methods' bodies hold calls too.
        for member in &mut class.body {
            member.visit_mut_with(&mut PrivateMethodCallRewriter {
                sites: self.sites,
                names: &names,
            });
        }
        self.recovered = true;
    }
}

/// Rewrites `check(this, brand, f).call(this, …args)` to `this.#f(…args)`.
struct PrivateMethodCallRewriter<'a, 'b> {
    sites: &'a PrivateCallSites<'b>,
    names: &'a HashMap<BindingKey, Atom>,
}

impl VisitMut for PrivateMethodCallRewriter<'_, '_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        let Expr::Call(call) = expr else {
            return;
        };
        let Some(PrivateOp::Method { method, .. }) = self.sites.private_method_call(call) else {
            return;
        };
        let Some(name) = self.names.get(&binding_key(method)).cloned() else {
            return;
        };
        let member = private_member_expr(name, DUMMY_SP);
        call.callee = Callee::Expr(Box::new(Expr::Member(member)));
        call.args.remove(0);
    }
}

struct BindingRefFinder<'a> {
    key: &'a BindingKey,
    found: bool,
}

impl Visit for BindingRefFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if binding_key(ident) == *self.key {
            self.found = true;
        }
    }
}

/// Whether the function names `name`'s symbol, other than as that very
/// binding when `same_binding_ok`: moved into the class, the reference would
/// read the class's inner name binding.
fn function_names_sym(
    function: &swc_core::ecma::ast::Function,
    name: &Ident,
    same_binding_ok: bool,
) -> bool {
    struct Finder<'a> {
        name: &'a Ident,
        same_binding_ok: bool,
        found: bool,
    }
    impl Visit for Finder<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.name.sym && !(self.same_binding_ok && ident.ctxt == self.name.ctxt)
            {
                self.found = true;
            }
        }
    }
    let mut finder = Finder {
        name,
        same_binding_ok,
        found: false,
    };
    function.visit_with(&mut finder);
    finder.found
}

/// The class's own name binding inside its body, and whether it is a class
/// declaration's (one binding with the outer name).
fn owner_class_inner_name(item: &ListItem) -> Option<(Ident, bool)> {
    use swc_core::ecma::ast::{DefaultDecl, ModuleDecl};
    let class_expr_name = |expr: &Expr| match crate::utils::paren::strip_parens(expr) {
        Expr::Class(class) => class.ident.clone().map(|ident| (ident, false)),
        _ => None,
    };
    match *item {
        ListItem::Stmt(Stmt::Decl(Decl::Class(decl)))
        | ListItem::ModuleDecl(ModuleDecl::ExportDecl(swc_core::ecma::ast::ExportDecl {
            decl: Decl::Class(decl),
            ..
        })) => Some((decl.ident.clone(), true)),
        ListItem::Stmt(Stmt::Decl(Decl::Var(var)))
        | ListItem::ModuleDecl(ModuleDecl::ExportDecl(swc_core::ecma::ast::ExportDecl {
            decl: Decl::Var(var),
            ..
        })) => var.decls.first()?.init.as_deref().and_then(class_expr_name),
        ListItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(export)) => match &export.decl {
            DefaultDecl::Class(class) => class.ident.clone().map(|ident| (ident, false)),
            _ => None,
        },
        ListItem::Stmt(Stmt::Expr(stmt)) => match crate::utils::paren::strip_parens(&stmt.expr) {
            Expr::Assign(assign) => class_expr_name(&assign.right),
            _ => None,
        },
        _ => None,
    }
}

/// The owner class of a statement, in the forms `private_map_class_owner`
/// accepts.
fn owner_class_in_stmt_mut(stmt: &mut Stmt) -> Option<&mut Class> {
    match stmt {
        Stmt::Decl(decl) => owner_class_in_decl_mut(decl),
        Stmt::Expr(stmt) => match crate::utils::paren::strip_parens_mut(&mut stmt.expr) {
            Expr::Assign(assign) => {
                match crate::utils::paren::strip_parens_mut(&mut assign.right) {
                    Expr::Class(expr) => Some(&mut expr.class),
                    _ => None,
                }
            }
            _ => None,
        },
        _ => None,
    }
}

fn owner_class_in_module_decl_mut(
    decl: &mut swc_core::ecma::ast::ModuleDecl,
) -> Option<&mut Class> {
    use swc_core::ecma::ast::{DefaultDecl, ModuleDecl};
    match decl {
        ModuleDecl::ExportDecl(export) => owner_class_in_decl_mut(&mut export.decl),
        ModuleDecl::ExportDefaultDecl(export) => match &mut export.decl {
            DefaultDecl::Class(expr) => Some(&mut expr.class),
            _ => None,
        },
        _ => None,
    }
}

fn owner_class_in_decl_mut(decl: &mut Decl) -> Option<&mut Class> {
    match decl {
        Decl::Class(decl) => Some(&mut decl.class),
        Decl::Var(var) if var.decls.len() == 1 => {
            match crate::utils::paren::strip_parens_mut(var.decls[0].init.as_mut()?) {
                Expr::Class(expr) => Some(&mut expr.class),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Drops the declarations and definitions of consumed brand and method
/// bindings: nothing references them any more.
fn retain_unconsumed_definition(stmt: &mut Stmt, consumed: &HashSet<BindingKey>) -> bool {
    match stmt {
        Stmt::Decl(Decl::Fn(decl)) => !consumed.contains(&binding_key(&decl.ident)),
        Stmt::Decl(Decl::Var(var)) => {
            var.decls.retain(|decl| {
                !matches!(&decl.name, Pat::Ident(binding) if consumed.contains(&binding_key(&binding.id)))
            });
            !var.decls.is_empty()
        }
        Stmt::Expr(expr) => !matches!(expr.expr.as_ref(), Expr::Assign(assign)
            if matches!(&assign.left, AssignTarget::Simple(SimpleAssignTarget::Ident(left))
                if consumed.contains(&binding_key(&left.id)))),
        _ => true,
    }
}

/// esbuild names `#x`'s function `x_fn` (`x_fn2` when the name is taken);
/// Babel and swc name it `_x`.
fn private_name_from_lowered_method_ident(sym: &Atom) -> Option<Atom> {
    let name = sym.as_ref();
    if let Some((base, suffix)) = name.rsplit_once("_fn") {
        if !base.is_empty()
            && suffix.chars().all(|ch| ch.is_ascii_digit())
            && is_identifier_name(base)
        {
            return Some(base.into());
        }
    }
    private_name_from_underscore_backing_ident(sym)
}

/// The private-field helper calls UnClassFields rewrites, from tsc and from
/// the producers in `lowered_private_field_helpers`.
struct PrivateCallSites<'a> {
    tsc_get: &'a PrivateHelperCallees,
    tsc_set: &'a PrivateHelperCallees,
    lowered: &'a HashMap<BindingKey, LoweredPrivateHelper>,
    /// `new WeakSet()` bindings: private-method brands.
    brand_sets: &'a HashSet<BindingKey>,
    unresolved_mark: Mark,
}

/// One operation on a backing map with `this` as the receiver.
enum PrivateOp<'a> {
    /// `map.set(this, value)` (tsc and older Babel) or an init helper. No
    /// value is esbuild's `__privateAdd(this, map)` for `#x;`.
    Init {
        map: &'a Ident,
        value: Option<&'a Expr>,
        through_helper: bool,
    },
    Get {
        map: &'a Ident,
        storage: PrivateStorage,
        through_helper: bool,
    },
    Set {
        map: &'a Ident,
        value: &'a Expr,
        storage: PrivateStorage,
        through_helper: bool,
    },
    /// A private method read through its brand check: tsc's
    /// `__classPrivateFieldGet(this, brand, "m", fn)` or a
    /// [`LoweredPrivateHelper::MethodGet`] call.
    Method {
        brand: &'a Ident,
        method: &'a Ident,
        tsc: bool,
    },
}

impl<'a> PrivateOp<'a> {
    fn map(&self) -> &'a Ident {
        match self {
            PrivateOp::Init { map, .. }
            | PrivateOp::Get { map, .. }
            | PrivateOp::Set { map, .. } => map,
            PrivateOp::Method { brand, .. } => brand,
        }
    }
}

impl PrivateCallSites<'_> {
    fn op<'c>(&self, call: &'c CallExpr) -> Option<PrivateOp<'c>> {
        if call.args.iter().any(|arg| arg.spread.is_some()) {
            return None;
        }
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let args: Vec<&Expr> = call.args.iter().map(|arg| arg.expr.as_ref()).collect();
        let is_this = |expr: &Expr| matches!(expr, Expr::This(_));
        let map_ident = |expr: &'c Expr| match expr {
            Expr::Ident(ident) => Some(ident),
            _ => None,
        };
        if let Expr::Member(member) = callee.as_ref() {
            if let (MemberProp::Ident(prop), Expr::Ident(map), [receiver, value]) =
                (&member.prop, member.obj.as_ref(), args.as_slice())
            {
                if prop.sym.as_ref() == "set" && is_this(receiver) {
                    return Some(PrivateOp::Init {
                        map,
                        value: Some(value),
                        through_helper: false,
                    });
                }
            }
        }
        if self.tsc_get.matches(callee) {
            if let [receiver, map, kind] = args.as_slice() {
                if is_this(receiver) && is_private_field_kind(kind) {
                    return Some(PrivateOp::Get {
                        map: map_ident(map)?,
                        storage: PrivateStorage::Raw,
                        through_helper: false,
                    });
                }
            }
            if let [receiver, brand, kind, method] = args.as_slice() {
                if is_this(receiver) && is_private_method_kind(kind) {
                    return Some(PrivateOp::Method {
                        brand: map_ident(brand)?,
                        method: map_ident(method)?,
                        tsc: true,
                    });
                }
            }
        }
        if self.tsc_set.matches(callee) {
            if let [receiver, map, value, kind] = args.as_slice() {
                if is_this(receiver) && is_private_field_kind(kind) {
                    return Some(PrivateOp::Set {
                        map: map_ident(map)?,
                        value,
                        storage: PrivateStorage::Raw,
                        through_helper: false,
                    });
                }
            }
        }
        let Expr::Ident(callee) = crate::utils::paren::strip_parens(callee) else {
            return None;
        };
        match *self.lowered.get(&binding_key(callee))? {
            LoweredPrivateHelper::AddBrand => None,
            LoweredPrivateHelper::Init => {
                let (receiver, map, value) = match args.as_slice() {
                    [receiver, map, value] => (receiver, map, Some(*value)),
                    [receiver, map] => (receiver, map, None),
                    _ => return None,
                };
                is_this(receiver).then_some(())?;
                Some(PrivateOp::Init {
                    map: map_ident(map)?,
                    value,
                    through_helper: true,
                })
            }
            LoweredPrivateHelper::Get {
                receiver,
                map,
                arity,
                storage,
            } => {
                (args.len() == arity && is_this(args[receiver])).then_some(())?;
                Some(PrivateOp::Get {
                    map: map_ident(args[map])?,
                    storage,
                    through_helper: true,
                })
            }
            LoweredPrivateHelper::Set {
                receiver,
                map,
                value,
                arity,
                storage,
            } => {
                (args.len() == arity && is_this(args[receiver])).then_some(())?;
                Some(PrivateOp::Set {
                    map: map_ident(args[map])?,
                    value: args[value],
                    storage,
                    through_helper: true,
                })
            }
            LoweredPrivateHelper::MethodGet {
                receiver,
                brand,
                method,
            } => {
                (args.len() == 3 && is_this(args[receiver])).then_some(())?;
                Some(PrivateOp::Method {
                    brand: map_ident(args[brand])?,
                    method: map_ident(args[method])?,
                    tsc: false,
                })
            }
        }
    }

    /// `check(this, brand, method).call(this, …)`, the lowered
    /// `this.#method(…)`, where `check` is a brand check.
    fn private_method_call<'c>(&self, call: &'c CallExpr) -> Option<PrivateOp<'c>> {
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Member(member) = crate::utils::paren::strip_parens(callee) else {
            return None;
        };
        if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "call") {
            return None;
        }
        let Expr::Call(check) = crate::utils::paren::strip_parens(&member.obj) else {
            return None;
        };
        let op @ PrivateOp::Method { .. } = self.op(check)? else {
            return None;
        };
        matches!(call.args.first(), Some(ExprOrSpread { spread: None, expr })
            if matches!(expr.as_ref(), Expr::This(_)))
        .then_some(op)
    }
}

impl PrivateCallSites<'_> {
    /// `add(this, brand)`, `__privateAdd(this, brand)`, or `brand.add(this)`:
    /// the constructor adds a private-method brand. Field promotion steps
    /// over one when the methods stay lowered.
    fn is_brand_add(&self, stmt: &Stmt) -> bool {
        self.brand_add_key(stmt).is_some()
    }

    /// The brand set a brand add (see `is_brand_add`) adds `this` to.
    fn brand_add_key(&self, stmt: &Stmt) -> Option<BindingKey> {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            return None;
        };
        let Expr::Call(call) = expr.as_ref() else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let brand_key = |expr: &Expr| match expr {
            Expr::Ident(id) if self.brand_sets.contains(&binding_key(id)) => Some(binding_key(id)),
            _ => None,
        };
        let args: Vec<&Expr> = call
            .args
            .iter()
            .map(|arg| arg.spread.is_none().then_some(arg.expr.as_ref()))
            .collect::<Option<_>>()?;
        match crate::utils::paren::strip_parens(callee) {
            Expr::Ident(helper) => {
                if !matches!(
                    self.lowered.get(&binding_key(helper)),
                    Some(LoweredPrivateHelper::AddBrand | LoweredPrivateHelper::Init)
                ) {
                    return None;
                }
                let [Expr::This(_), brand] = args.as_slice() else {
                    return None;
                };
                brand_key(brand)
            }
            Expr::Member(member) => {
                (matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "add")
                    && matches!(args.as_slice(), [Expr::This(_)]))
                .then_some(())?;
                brand_key(&member.obj)
            }
            _ => None,
        }
    }
}

/// A field value whose evaluation cannot observe a private-method brand:
/// literals, function values, `void` of a literal, empty builtin
/// collections, and arrays and plain objects of those.
fn is_inert_field_value(expr: &Expr, unresolved_mark: Mark) -> bool {
    use swc_core::ecma::ast::UnaryOp;
    match crate::utils::paren::strip_parens(expr) {
        Expr::Lit(_) | Expr::Fn(_) | Expr::Arrow(_) => true,
        Expr::Tpl(tpl) => tpl.exprs.is_empty(),
        Expr::Ident(id) => id.sym.as_ref() == "undefined" && id.ctxt.outer() == unresolved_mark,
        Expr::Unary(unary) => {
            matches!(unary.op, UnaryOp::Void | UnaryOp::Bang | UnaryOp::Minus)
                && matches!(crate::utils::paren::strip_parens(&unary.arg), Expr::Lit(_))
        }
        Expr::Array(array) => array.elems.iter().all(|elem| match elem {
            None => true,
            Some(elem) => {
                elem.spread.is_none() && is_inert_field_value(&elem.expr, unresolved_mark)
            }
        }),
        Expr::Object(object) => object.props.iter().all(|prop| match prop {
            PropOrSpread::Prop(prop) => match prop.as_ref() {
                Prop::KeyValue(KeyValueProp { key, value }) => {
                    !matches!(key, PropName::Computed(_))
                        && is_inert_field_value(value, unresolved_mark)
                }
                _ => false,
            },
            PropOrSpread::Spread(_) => false,
        }),
        Expr::New(new_expr) => {
            new_expr.args.as_ref().is_none_or(|args| args.is_empty())
                && matches!(new_expr.callee.as_ref(), Expr::Ident(id)
                    if id.ctxt.outer() == unresolved_mark
                        && matches!(id.sym.as_ref(), "Map" | "Set" | "WeakMap" | "WeakSet"))
        }
        _ => false,
    }
}

/// How a class uses one backing map, when every reference is a supported
/// operation with `this` as the receiver.
struct PrivateMapUse {
    inits: usize,
    storage: Option<PrivateStorage>,
    through_helper: bool,
}

fn class_private_map_use(
    class: &Class,
    map_key: &BindingKey,
    sites: &PrivateCallSites,
) -> Option<PrivateMapUse> {
    struct UseFinder<'a> {
        map_key: &'a BindingKey,
        sites: &'a PrivateCallSites<'a>,
        usage: PrivateMapUse,
        unsupported: bool,
    }

    impl UseFinder<'_> {
        /// A read in a position where `this.#x` would mean something else:
        /// a callee or tag (it would bind `this`), or a `delete` operand (a
        /// syntax error on a private reference).
        fn flag_read_in(&mut self, expr: &Expr) {
            if let Expr::Call(call) = crate::utils::paren::strip_parens(expr) {
                if matches!(self.sites.op(call), Some(PrivateOp::Get { map, .. })
                    if binding_key(map) == *self.map_key)
                {
                    self.unsupported = true;
                }
            }
        }
    }

    impl Visit for UseFinder<'_> {
        fn visit_tagged_tpl(&mut self, tagged: &swc_core::ecma::ast::TaggedTpl) {
            self.flag_read_in(&tagged.tag);
            tagged.visit_children_with(self);
        }

        fn visit_opt_call(&mut self, call: &swc_core::ecma::ast::OptCall) {
            self.flag_read_in(&call.callee);
            call.visit_children_with(self);
        }

        fn visit_unary_expr(&mut self, unary: &swc_core::ecma::ast::UnaryExpr) {
            if unary.op == swc_core::ecma::ast::UnaryOp::Delete {
                self.flag_read_in(&unary.arg);
            }
            unary.visit_children_with(self);
        }

        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                self.flag_read_in(callee);
            }
            let Some(op) = self.sites.op(call) else {
                call.visit_children_with(self);
                return;
            };
            if binding_key(op.map()) != *self.map_key {
                call.visit_children_with(self);
                return;
            }
            let (value, storage, through_helper) = match op {
                PrivateOp::Init {
                    value,
                    through_helper,
                    ..
                } => {
                    self.usage.inits += 1;
                    (value, None, through_helper)
                }
                PrivateOp::Get {
                    storage,
                    through_helper,
                    ..
                } => (None, Some(storage), through_helper),
                PrivateOp::Set {
                    value,
                    storage,
                    through_helper,
                    ..
                } => (Some(value), Some(storage), through_helper),
                // A map passed as a brand is not a field.
                PrivateOp::Method { .. } => {
                    self.unsupported = true;
                    return;
                }
            };
            self.usage.through_helper |= through_helper;
            if let Some(storage) = storage {
                match self.usage.storage {
                    Some(seen) if seen != storage => self.unsupported = true,
                    _ => self.usage.storage = Some(storage),
                }
            }
            if let Some(value) = value {
                value.visit_with(self);
            }
        }

        fn visit_ident(&mut self, ident: &Ident) {
            if binding_key(ident) == *self.map_key {
                self.unsupported = true;
            }
        }
    }

    let mut finder = UseFinder {
        map_key,
        sites,
        usage: PrivateMapUse {
            inits: 0,
            storage: None,
            through_helper: false,
        },
        unsupported: false,
    };
    class.visit_with(&mut finder);
    (!finder.unsupported).then_some(finder.usage)
}

/// `{ writable: true, value }`, the descriptor swc and older Babel store:
/// its value.
fn private_field_descriptor_value(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Object(object) = crate::utils::paren::strip_parens(expr) else {
        return None;
    };
    let mut writable = false;
    let mut value = None;
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
        match prop_name_str(key).as_deref() {
            Some("writable") if !writable && is_true_literal(prop_value) => writable = true,
            Some("value") if value.is_none() => value = Some(prop_value.clone()),
            _ => return None,
        }
    }
    if writable {
        value
    } else {
        None
    }
}

fn class_private_names(class: &Class) -> HashSet<Atom> {
    struct Names(HashSet<Atom>);
    impl Visit for Names {
        fn visit_private_name(&mut self, name: &PrivateName) {
            self.0.insert(name.name.clone());
        }
    }
    let mut names = Names(HashSet::default());
    class.visit_with(&mut names);
    names.0
}

/// Promotes the constructor's leading private field initializers, and, in a
/// base class, the public field initializers among them (see
/// `extract_instance_field_initializer`), in their order. Nothing changes
/// unless a private field is promoted: public fields alone are left to
/// `promote_constructor_field_assignments`.
fn promote_private_field_initializers(
    class: &mut Class,
    private_maps: &HashMap<BindingKey, Atom>,
    single_owner_maps: &HashSet<BindingKey>,
    sites: &PrivateCallSites,
    define_property_helpers: &HashSet<BindingKey>,
) -> HashMap<BindingKey, Atom> {
    let Some(ctor_index) = class
        .body
        .iter()
        .position(|member| matches!(member, ClassMember::Constructor(_)))
    else {
        return HashMap::default();
    };

    let map_uses: HashMap<BindingKey, PrivateMapUse> = single_owner_maps
        .iter()
        .filter_map(|key| {
            class_private_map_use(class, key, sites).map(|usage| (key.clone(), usage))
        })
        .collect();
    let mut taken_names = class_private_names(class);
    let is_derived = class.super_class.is_some();
    let assigned_keys = AssignableKeys::of(class);

    let (private_props, promoted_maps, remove_empty_ctor) = {
        let ClassMember::Constructor(ctor) = &mut class.body[ctor_index] else {
            return HashMap::default();
        };
        let Some(body) = &mut ctor.body else {
            return HashMap::default();
        };
        let mut blocked_bindings = constructor_blocked_bindings(&ctor.params);
        // A derived class defines its fields when `super(…)` returns: the
        // initializers follow the constructor's one top-level super call, and
        // may not read what the statements before it declare.
        let start = if is_derived {
            let Some(super_index) = single_top_level_super_call(&body.stmts) else {
                return HashMap::default();
            };
            collect_declared_bindings(&body.stmts[..super_index], &mut blocked_bindings);
            super_index + 1
        } else {
            0
        };
        let mut private_props = Vec::new();
        let mut promoted_maps = HashMap::default();
        let mut consumed = Vec::new();
        // Private methods get their brand before any field. A field promoted
        // past a lowered brand add now runs first, so its value must not be
        // able to observe the brand.
        let mut past_brand_add = false;
        // tsc assigns public fields (`this.x = 1`) when it does not define
        // them. A private field may be promoted past such an assignment when
        // neither can observe the other: both values are inert, and the
        // assignment runs no setter (see `is_inert_this_assignment`). A public
        // field may not, as that would reorder the own keys.
        let mut past_assignment = false;

        for (index, stmt) in body.stmts.iter().enumerate().skip(start) {
            if sites.is_brand_add(stmt) {
                past_brand_add = true;
                continue;
            }
            if !is_derived && is_inert_this_assignment(stmt, &assigned_keys, sites.unresolved_mark)
            {
                past_assignment = true;
                continue;
            }
            let Some((map_key, value, init_through_helper)) =
                extract_private_field_initializer(stmt, private_maps, sites)
            else {
                // The same guards as `promote_constructor_field_assignments`,
                // which handles public fields only in a base class.
                let Some((key, value)) = (!is_derived && !past_assignment)
                    .then(|| {
                        extract_instance_field_initializer(
                            stmt,
                            define_property_helpers,
                            sites.unresolved_mark,
                        )
                    })
                    .flatten()
                else {
                    break;
                };
                if expr_uses_blocked_binding(&value, &blocked_bindings)
                    || (past_brand_add && !is_inert_field_value(&value, sites.unresolved_mark))
                {
                    break;
                }
                private_props.push(public_class_prop(key, value));
                consumed.push(index);
                continue;
            };
            let Some(usage) = map_uses.get(&map_key) else {
                break;
            };
            if usage.inits != 1 {
                break;
            }
            let value = match (usage.storage, value) {
                (Some(PrivateStorage::Descriptor), value) => {
                    let Some(value) = value.as_deref().and_then(private_field_descriptor_value)
                    else {
                        break;
                    };
                    Some(value)
                }
                // An init helper with no read or write is shared by Babel
                // (value) and swc (descriptor); a descriptor-shaped value
                // cannot tell them apart.
                (None, Some(value))
                    if init_through_helper && private_field_descriptor_value(&value).is_some() =>
                {
                    break;
                }
                (_, value) => value,
            };
            if value
                .as_deref()
                .is_some_and(|value| expr_uses_blocked_binding(value, &blocked_bindings))
            {
                break;
            }
            if (past_brand_add || past_assignment)
                && !value
                    .as_deref()
                    .is_none_or(|value| is_inert_field_value(value, sites.unresolved_mark))
            {
                break;
            }
            let backing = &private_maps[&map_key];
            let private_name =
                if usage.through_helper || usage.storage == Some(PrivateStorage::Descriptor) {
                    private_name_from_underscore_backing_ident(backing)
                } else {
                    private_name_from_tsc_backing_ident(backing)
                };
            let Some(private_name) = private_name else {
                break;
            };
            if !taken_names.insert(private_name.clone()) {
                break;
            }
            private_props.push(ClassMember::PrivateProp(PrivateProp {
                span: DUMMY_SP,
                ctxt: SyntaxContext::empty(),
                key: PrivateName {
                    span: DUMMY_SP,
                    name: private_name.clone(),
                },
                value,
                type_ann: None,
                is_static: false,
                decorators: Vec::new(),
                accessibility: None,
                is_optional: false,
                is_override: false,
                readonly: false,
                definite: false,
            }));
            promoted_maps.insert(map_key, private_name);
            consumed.push(index);
        }

        if promoted_maps.is_empty() {
            return HashMap::default();
        }

        for index in consumed.into_iter().rev() {
            body.stmts.remove(index);
        }
        let remove_empty_ctor = !is_derived && body.stmts.is_empty() && ctor.params.is_empty();
        (private_props, promoted_maps, remove_empty_ctor)
    };

    if remove_empty_ctor {
        class.body.remove(ctor_index);
    }
    for (offset, prop) in private_props.into_iter().enumerate() {
        class.body.insert(ctor_index + offset, prop);
    }
    promoted_maps
}

fn extract_private_field_initializer(
    stmt: &Stmt,
    private_maps: &HashMap<BindingKey, Atom>,
    sites: &PrivateCallSites,
) -> Option<(BindingKey, Option<Box<Expr>>, bool)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let PrivateOp::Init {
        map,
        value,
        through_helper,
    } = sites.op(call)?
    else {
        return None;
    };
    let map_key = binding_key(map);
    private_maps.contains_key(&map_key).then_some(())?;
    Some((
        map_key,
        value.map(|value| Box::new(value.clone())),
        through_helper,
    ))
}

struct PrivateFieldAccessRewriter<'a> {
    promoted_maps: &'a HashMap<BindingKey, Atom>,
    sites: &'a PrivateCallSites<'a>,
}

impl VisitMut for PrivateFieldAccessRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else {
            return;
        };
        let (map, value) = match self.sites.op(call) {
            Some(PrivateOp::Get { map, .. }) => (map, None),
            Some(PrivateOp::Set { map, value, .. }) => (map, Some(Box::new(value.clone()))),
            _ => return,
        };
        let Some(private_name) = self.promoted_maps.get(&binding_key(map)).cloned() else {
            return;
        };
        let original_span = call.span;
        let member = private_member_expr(private_name, original_span);
        if let Some(value) = value {
            *expr = Expr::Assign(AssignExpr {
                span: original_span,
                op: AssignOp::Assign,
                left: AssignTarget::Simple(SimpleAssignTarget::Member(member)),
                right: value,
            });
        } else {
            *expr = Expr::Member(member);
        }
    }
}

fn is_private_field_kind(expr: &Expr) -> bool {
    super::expr_utils::string_lit_value(expr).is_some_and(|kind| kind == "f")
}

fn is_private_method_kind(expr: &Expr) -> bool {
    super::expr_utils::string_lit_value(expr).is_some_and(|kind| kind == "m")
}

fn private_member_expr(private_name: Atom, span: swc_core::common::Span) -> MemberExpr {
    MemberExpr {
        span,
        obj: Box::new(Expr::This(swc_core::ecma::ast::ThisExpr { span: DUMMY_SP })),
        prop: MemberProp::PrivateName(PrivateName {
            span: DUMMY_SP,
            name: private_name,
        }),
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
            class_props.push(public_class_prop(key, value));
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

/// The instance accessor keys a class declares, which an assignment to
/// `this` could run.
struct AssignableKeys {
    accessors: HashSet<Atom>,
    computed_accessor: bool,
}

impl AssignableKeys {
    fn of(class: &Class) -> Self {
        let mut keys = Self {
            accessors: HashSet::default(),
            computed_accessor: false,
        };
        for member in &class.body {
            let method = match member {
                ClassMember::Method(method) => method,
                ClassMember::AutoAccessor(_) => {
                    keys.computed_accessor = true;
                    continue;
                }
                _ => continue,
            };
            if method.is_static || method.kind == swc_core::ecma::ast::MethodKind::Method {
                continue;
            }
            match prop_name_str(&method.key) {
                Some(name) => {
                    keys.accessors.insert(Atom::from(name));
                }
                None => keys.computed_accessor = true,
            }
        }
        keys
    }
}

/// `this.x = value;` in a base class constructor, where the assignment can
/// run no code: the value is inert, the class declares no `x` accessor, and
/// `x` is not `__proto__` (under the Execution Environment Baseline no other
/// inherited setter exists).
fn is_inert_this_assignment(stmt: &Stmt, keys: &AssignableKeys, unresolved_mark: Mark) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return false;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return false;
    };
    let MemberProp::Ident(prop) = &member.prop else {
        return false;
    };
    assign.op == AssignOp::Assign
        && matches!(member.obj.as_ref(), Expr::This(_))
        && prop.sym.as_ref() != "__proto__"
        && !keys.computed_accessor
        && !keys.accessors.contains(&prop.sym)
        && is_inert_field_value(&assign.right, unresolved_mark)
}

fn public_class_prop(key: PropName, value: Box<Expr>) -> ClassMember {
    ClassMember::ClassProp(ClassProp {
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
    })
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

/// The index of the constructor statement `super(…);`, when it is the only
/// super call in the constructor.
fn single_top_level_super_call(stmts: &[Stmt]) -> Option<usize> {
    struct SuperCalls(usize);
    impl Visit for SuperCalls {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if matches!(call.callee, Callee::Super(_)) {
                self.0 += 1;
            }
            call.visit_children_with(self);
        }
        // A nested class has its own super calls; a nested function cannot
        // call `super`.
        fn visit_class(&mut self, _: &Class) {}
        fn visit_function(&mut self, _: &swc_core::ecma::ast::Function) {}
    }
    let mut calls = SuperCalls(0);
    stmts.visit_with(&mut calls);
    if calls.0 != 1 {
        return None;
    }
    stmts.iter().position(|stmt| {
        matches!(stmt, Stmt::Expr(ExprStmt { expr, .. })
            if matches!(expr.as_ref(), Expr::Call(call) if matches!(call.callee, Callee::Super(_))))
    })
}

/// Every binding the statements declare, at any depth.
fn collect_declared_bindings(stmts: &[Stmt], bindings: &mut Vec<(Atom, SyntaxContext)>) {
    struct Declared<'a>(&'a mut Vec<(Atom, SyntaxContext)>);
    impl Visit for Declared<'_> {
        fn visit_binding_ident(&mut self, binding: &BindingIdent) {
            self.0.push((binding.id.sym.clone(), binding.id.ctxt));
        }
        fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
            self.0.push((decl.ident.sym.clone(), decl.ident.ctxt));
            decl.visit_children_with(self);
        }
        fn visit_class_decl(&mut self, decl: &swc_core::ecma::ast::ClassDecl) {
            self.0.push((decl.ident.sym.clone(), decl.ident.ctxt));
            decl.visit_children_with(self);
        }
    }
    stmts.visit_with(&mut Declared(bindings));
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
