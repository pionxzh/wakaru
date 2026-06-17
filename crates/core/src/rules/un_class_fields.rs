use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BindingIdent, CallExpr, Callee, Class, ClassMember,
    ClassProp, Decl, Expr, ExprOrSpread, ExprStmt, Ident, IdentName, KeyValueProp, Lit, MemberExpr,
    MemberProp, Module, ModuleItem, Pat, PrivateName, PrivateProp, Prop, PropName, PropOrSpread,
    SimpleAssignTarget, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::helper_matcher::binding_key;
use super::transpiler_helper_utils::{
    BindingKey, LocalHelperContext, TranspilerHelperKind, TsHelperKind,
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
    private_get_helpers: HashSet<BindingKey>,
    private_set_helpers: HashSet<BindingKey>,
    private_single_owner_maps: HashSet<BindingKey>,
    consumed_private_maps: HashSet<BindingKey>,
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
            private_maps: HashMap::new(),
            private_get_helpers: HashSet::new(),
            private_set_helpers: HashSet::new(),
            private_single_owner_maps: HashSet::new(),
            consumed_private_maps: HashSet::new(),
        }
    }

    pub(crate) fn run_with_helpers(
        &mut self,
        module: &mut Module,
        local_helpers: &LocalHelperContext,
    ) {
        let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::DefineProperty);
        let previous_helpers = std::mem::replace(
            &mut self.define_property_helpers,
            helpers.keys().cloned().collect(),
        );
        let private_maps = collect_private_weak_maps(module, self.unresolved_mark);
        let previous_private_maps = std::mem::replace(&mut self.private_maps, private_maps);
        let previous_private_get_helpers = std::mem::replace(
            &mut self.private_get_helpers,
            local_helpers.ts_helpers_of_kind(TsHelperKind::ClassPrivateFieldGet),
        );
        let previous_private_set_helpers = std::mem::replace(
            &mut self.private_set_helpers,
            local_helpers.ts_helpers_of_kind(TsHelperKind::ClassPrivateFieldSet),
        );
        let previous_private_single_owner_maps = std::mem::replace(
            &mut self.private_single_owner_maps,
            collect_single_owner_private_maps(module, &self.private_maps, self.unresolved_mark),
        );
        let previous_consumed_private_maps = std::mem::take(&mut self.consumed_private_maps);

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

        let private_helpers: HashSet<BindingKey> = self
            .private_get_helpers
            .iter()
            .chain(&self.private_set_helpers)
            .cloned()
            .collect();
        if !private_helpers.is_empty() {
            let removable_private_helpers: HashSet<BindingKey> = private_helpers
                .iter()
                .filter(|key| !private_helper_has_remaining_refs(module, key))
                .cloned()
                .collect();
            if !removable_private_helpers.is_empty() {
                remove_private_helper_declarations(module, &removable_private_helpers);
            }
        }

        if !helpers.is_empty() {
            local_helpers.remove_helpers_with_dependencies(module, helpers);
        }
        self.define_property_helpers = previous_helpers;
        self.private_maps = previous_private_maps;
        self.private_get_helpers = previous_private_get_helpers;
        self.private_set_helpers = previous_private_set_helpers;
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
            let promoted_private_maps = promote_private_field_initializers(
                class,
                &self.private_maps,
                &self.private_single_owner_maps,
                &self.private_get_helpers,
                &self.private_set_helpers,
            );
            if !promoted_private_maps.is_empty() {
                rewrite_private_field_accesses(
                    class,
                    &promoted_private_maps,
                    &self.private_maps,
                    &self.private_get_helpers,
                    &self.private_set_helpers,
                );
                self.consumed_private_maps.extend(promoted_private_maps);
            }
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

fn collect_private_weak_maps(module: &Module, unresolved_mark: Mark) -> HashMap<BindingKey, Atom> {
    let mut maps = HashMap::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
                for decl in &var_decl.decls {
                    let Pat::Ident(BindingIdent { id, .. }) = &decl.name else {
                        continue;
                    };
                    if decl
                        .init
                        .as_deref()
                        .is_some_and(|init| is_new_weak_map_expression(init, unresolved_mark))
                    {
                        if let Some(name) = private_name_from_backing_ident(&id.sym) {
                            maps.insert(binding_key(id), name);
                        }
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Expr(expr_stmt)) => {
                collect_private_weak_map_assignments(&expr_stmt.expr, &mut maps, unresolved_mark);
            }
            _ => {}
        }
    }
    maps
}

fn collect_single_owner_private_maps(
    module: &Module,
    private_maps: &HashMap<BindingKey, Atom>,
    unresolved_mark: Mark,
) -> HashSet<BindingKey> {
    if private_maps.is_empty() {
        return HashSet::new();
    }

    let mut class_counts: HashMap<BindingKey, usize> = HashMap::new();
    let mut counter = PrivateMapClassConsumerCounter {
        private_maps,
        class_counts: &mut class_counts,
    };
    module.visit_with(&mut counter);

    let mut outside_finder = OutsidePrivateMapRefFinder {
        private_maps,
        unresolved_mark,
        refs: HashSet::new(),
    };
    module.visit_with(&mut outside_finder);

    private_maps
        .keys()
        .filter(|key| class_counts.get(*key).copied() == Some(1))
        .filter(|key| !outside_finder.refs.contains(*key))
        .cloned()
        .collect()
}

struct PrivateMapClassConsumerCounter<'a> {
    private_maps: &'a HashMap<BindingKey, Atom>,
    class_counts: &'a mut HashMap<BindingKey, usize>,
}

impl Visit for PrivateMapClassConsumerCounter<'_> {
    fn visit_class(&mut self, class: &Class) {
        let mut collector = PrivateMapRefCollector {
            private_maps: self.private_maps,
            refs: HashSet::new(),
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

struct OutsidePrivateMapRefFinder<'a> {
    private_maps: &'a HashMap<BindingKey, Atom>,
    unresolved_mark: Mark,
    refs: HashSet<BindingKey>,
}

impl Visit for OutsidePrivateMapRefFinder<'_> {
    fn visit_module(&mut self, module: &Module) {
        for item in &module.body {
            match item {
                ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
                    for decl in &var_decl.decls {
                        if let Some(init) = &decl.init {
                            init.visit_with(self);
                        }
                    }
                }
                ModuleItem::Stmt(Stmt::Expr(expr_stmt)) => {
                    visit_expr_skipping_module_private_initializers(
                        &expr_stmt.expr,
                        self.unresolved_mark,
                        self,
                    );
                }
                _ => item.visit_with(self),
            }
        }
    }

    fn visit_class(&mut self, _class: &Class) {}

    fn visit_var_declarator(&mut self, decl: &swc_core::ecma::ast::VarDeclarator) {
        if let Some(init) = &decl.init {
            init.visit_with(self);
        }
    }

    fn visit_ident(&mut self, ident: &Ident) {
        let key = binding_key(ident);
        if self.private_maps.contains_key(&key) {
            self.refs.insert(key);
        }
    }
}

fn visit_expr_skipping_module_private_initializers<V: Visit>(
    expr: &Expr,
    unresolved_mark: Mark,
    visitor: &mut V,
) {
    match expr {
        Expr::Seq(seq) => {
            for expr in &seq.exprs {
                visit_expr_skipping_module_private_initializers(expr, unresolved_mark, visitor);
            }
        }
        _ if private_weak_map_assignment_key(expr, unresolved_mark).is_some() => {}
        _ => expr.visit_with(visitor),
    }
}

fn collect_private_weak_map_assignments(
    expr: &Expr,
    maps: &mut HashMap<BindingKey, Atom>,
    unresolved_mark: Mark,
) {
    match expr {
        Expr::Seq(seq) => {
            for expr in &seq.exprs {
                collect_private_weak_map_assignments(expr, maps, unresolved_mark);
            }
        }
        Expr::Assign(assign) if assign.op == AssignOp::Assign => {
            let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left else {
                return;
            };
            if !is_new_weak_map_expression(&assign.right, unresolved_mark) {
                return;
            }
            if let Some(name) = private_name_from_backing_ident(&left.id.sym) {
                maps.insert(binding_key(&left.id), name);
            }
        }
        _ => {}
    }
}

fn private_name_from_backing_ident(sym: &Atom) -> Option<Atom> {
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
        fn visit_module(&mut self, module: &Module) {
            for item in &module.body {
                match item {
                    ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
                        for decl in &var_decl.decls {
                            if let Pat::Ident(BindingIdent { id, .. }) = &decl.name {
                                if binding_key(id) == *self.key
                                    && decl.init.as_deref().is_none_or(|init| {
                                        is_new_weak_map_expression(init, self.unresolved_mark)
                                    })
                                {
                                    continue;
                                }
                            }
                            decl.visit_with(self);
                        }
                    }
                    ModuleItem::Stmt(Stmt::Expr(expr_stmt)) => {
                        visit_expr_skipping_module_private_initializers(
                            &expr_stmt.expr,
                            self.unresolved_mark,
                            self,
                        );
                    }
                    _ => item.visit_with(self),
                }
            }
        }

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

fn remove_private_map_initializers(
    module: &mut Module,
    removable: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) {
    module.body.retain_mut(|item| match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
            var_decl.decls.retain(|decl| {
                let Pat::Ident(BindingIdent { id, .. }) = &decl.name else {
                    return true;
                };
                let key = binding_key(id);
                if !removable.contains(&key) {
                    return true;
                }
                !decl
                    .init
                    .as_deref()
                    .is_none_or(|init| is_new_weak_map_expression(init, unresolved_mark))
            });
            !var_decl.decls.is_empty()
        }
        ModuleItem::Stmt(Stmt::Expr(expr_stmt)) => {
            remove_private_weak_map_assignment_expr(&mut expr_stmt.expr, removable, unresolved_mark)
        }
        _ => true,
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

fn private_helper_has_remaining_refs(module: &Module, key: &BindingKey) -> bool {
    struct RefFinder<'a> {
        key: &'a BindingKey,
        found: bool,
    }

    impl Visit for RefFinder<'_> {
        fn visit_fn_decl(&mut self, fn_decl: &swc_core::ecma::ast::FnDecl) {
            if binding_key(&fn_decl.ident) == *self.key {
                return;
            }
            fn_decl.visit_children_with(self);
        }

        fn visit_var_declarator(&mut self, decl: &swc_core::ecma::ast::VarDeclarator) {
            if let Pat::Ident(BindingIdent { id, .. }) = &decl.name {
                if binding_key(id) == *self.key {
                    return;
                }
            }
            decl.visit_children_with(self);
        }

        fn visit_ident(&mut self, ident: &Ident) {
            if binding_key(ident) == *self.key {
                self.found = true;
            }
        }
    }

    let mut finder = RefFinder { key, found: false };
    module.visit_with(&mut finder);
    finder.found
}

fn remove_private_helper_declarations(module: &mut Module, removable: &HashSet<BindingKey>) {
    module.body.retain_mut(|item| match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
            !removable.contains(&binding_key(&fn_decl.ident))
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
            var_decl.decls.retain(|decl| {
                let Pat::Ident(BindingIdent { id, .. }) = &decl.name else {
                    return true;
                };
                !removable.contains(&binding_key(id))
            });
            !var_decl.decls.is_empty()
        }
        _ => true,
    });
}

fn class_has_unsupported_private_map_refs(
    class: &Class,
    map_key: &BindingKey,
    get_helpers: &HashSet<BindingKey>,
    set_helpers: &HashSet<BindingKey>,
) -> bool {
    struct UnsupportedRefFinder<'a> {
        map_key: &'a BindingKey,
        get_helpers: &'a HashSet<BindingKey>,
        set_helpers: &'a HashSet<BindingKey>,
        found: bool,
    }

    impl Visit for UnsupportedRefFinder<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if self.is_supported_initializer(call) {
                if let Some(value) = call.args.get(1) {
                    value.expr.visit_with(self);
                }
                return;
            }
            if let Some(value_index) = self.supported_helper_value_index(call) {
                if let Some(value) = value_index.and_then(|index| call.args.get(index)) {
                    value.expr.visit_with(self);
                }
                return;
            }
            call.visit_children_with(self);
        }

        fn visit_ident(&mut self, ident: &Ident) {
            if binding_key(ident) == *self.map_key {
                self.found = true;
            }
        }
    }

    impl UnsupportedRefFinder<'_> {
        fn is_supported_initializer(&self, call: &CallExpr) -> bool {
            if call.args.len() != 2 || call.args.iter().any(|arg| arg.spread.is_some()) {
                return false;
            }
            let Callee::Expr(callee) = &call.callee else {
                return false;
            };
            let Expr::Member(member) = callee.as_ref() else {
                return false;
            };
            if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "set") {
                return false;
            }
            let Expr::Ident(map_ident) = member.obj.as_ref() else {
                return false;
            };
            if binding_key(map_ident) != *self.map_key {
                return false;
            }
            matches!(call.args[0].expr.as_ref(), Expr::This(_))
        }

        fn supported_helper_value_index(&self, call: &CallExpr) -> Option<Option<usize>> {
            if call.args.iter().any(|arg| arg.spread.is_some()) {
                return None;
            }
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            let Expr::Ident(callee_ident) = callee.as_ref() else {
                return None;
            };
            let callee_key = binding_key(callee_ident);
            if self.get_helpers.contains(&callee_key) {
                let [ExprOrSpread { expr: receiver, .. }, ExprOrSpread { expr: map, .. }, ExprOrSpread { expr: kind, .. }] =
                    call.args.as_slice()
                else {
                    return None;
                };
                if matches!(receiver.as_ref(), Expr::This(_))
                    && map_matches_binding(map, self.map_key)
                    && is_private_field_kind(kind)
                {
                    return Some(None);
                }
            }
            if self.set_helpers.contains(&callee_key) {
                let [ExprOrSpread { expr: receiver, .. }, ExprOrSpread { expr: map, .. }, ExprOrSpread { expr: _value, .. }, ExprOrSpread { expr: kind, .. }] =
                    call.args.as_slice()
                else {
                    return None;
                };
                if matches!(receiver.as_ref(), Expr::This(_))
                    && map_matches_binding(map, self.map_key)
                    && is_private_field_kind(kind)
                {
                    return Some(Some(2));
                }
            }
            None
        }
    }

    let mut finder = UnsupportedRefFinder {
        map_key,
        get_helpers,
        set_helpers,
        found: false,
    };
    class.visit_with(&mut finder);
    finder.found
}

fn map_matches_binding(expr: &Expr, key: &BindingKey) -> bool {
    matches!(expr, Expr::Ident(ident) if binding_key(ident) == *key)
}

fn promote_private_field_initializers(
    class: &mut Class,
    private_maps: &HashMap<BindingKey, Atom>,
    single_owner_maps: &HashSet<BindingKey>,
    get_helpers: &HashSet<BindingKey>,
    set_helpers: &HashSet<BindingKey>,
) -> HashSet<BindingKey> {
    let Some(ctor_index) = class
        .body
        .iter()
        .position(|member| matches!(member, ClassMember::Constructor(_)))
    else {
        return HashSet::new();
    };

    let unsupported_private_maps: HashSet<BindingKey> = private_maps
        .keys()
        .filter(|key| class_has_unsupported_private_map_refs(class, key, get_helpers, set_helpers))
        .cloned()
        .collect();

    let (private_props, promoted_maps, remove_empty_ctor) = {
        let ClassMember::Constructor(ctor) = &mut class.body[ctor_index] else {
            return HashSet::new();
        };
        let Some(body) = &mut ctor.body else {
            return HashSet::new();
        };
        let blocked_bindings = constructor_blocked_bindings(&ctor.params);
        let mut private_props = Vec::new();
        let mut promoted_maps = HashSet::new();
        let mut consumed = 0;

        for stmt in &body.stmts {
            let Some((map_key, private_name, value)) =
                extract_private_field_initializer(stmt, private_maps)
            else {
                break;
            };
            if expr_uses_blocked_binding(&value, &blocked_bindings) {
                break;
            }
            if !single_owner_maps.contains(&map_key) {
                break;
            }
            if unsupported_private_maps.contains(&map_key) {
                break;
            }
            private_props.push(ClassMember::PrivateProp(PrivateProp {
                span: DUMMY_SP,
                ctxt: SyntaxContext::empty(),
                key: PrivateName {
                    span: DUMMY_SP,
                    name: private_name,
                },
                value: Some(value),
                type_ann: None,
                is_static: false,
                decorators: Vec::new(),
                accessibility: None,
                is_optional: false,
                is_override: false,
                readonly: false,
                definite: false,
            }));
            promoted_maps.insert(map_key);
            consumed += 1;
        }

        if private_props.is_empty() {
            return HashSet::new();
        }

        body.stmts.drain(0..consumed);
        let remove_empty_ctor = body.stmts.is_empty() && ctor.params.is_empty();
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
) -> Option<(BindingKey, Atom, Box<Expr>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    if call.args.len() != 2 || call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "set") {
        return None;
    }
    let Expr::Ident(map_ident) = member.obj.as_ref() else {
        return None;
    };
    let map_key = binding_key(map_ident);
    let private_name = private_maps.get(&map_key)?;
    let [ExprOrSpread { expr: receiver, .. }, ExprOrSpread { expr: value, .. }] =
        call.args.as_slice()
    else {
        return None;
    };
    if !matches!(receiver.as_ref(), Expr::This(_)) {
        return None;
    }
    Some((map_key, private_name.clone(), value.clone()))
}

fn rewrite_private_field_accesses(
    class: &mut Class,
    promoted_maps: &HashSet<BindingKey>,
    private_maps: &HashMap<BindingKey, Atom>,
    get_helpers: &HashSet<BindingKey>,
    set_helpers: &HashSet<BindingKey>,
) {
    class.visit_mut_with(&mut PrivateFieldAccessRewriter {
        promoted_maps,
        private_maps,
        get_helpers,
        set_helpers,
    });
}

struct PrivateFieldAccessRewriter<'a> {
    promoted_maps: &'a HashSet<BindingKey>,
    private_maps: &'a HashMap<BindingKey, Atom>,
    get_helpers: &'a HashSet<BindingKey>,
    set_helpers: &'a HashSet<BindingKey>,
}

impl VisitMut for PrivateFieldAccessRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else {
            return;
        };
        let Some((private_name, value)) = self.extract_private_helper_access(call) else {
            return;
        };
        let member = private_member_expr(private_name);
        if let Some(value) = value {
            *expr = Expr::Assign(AssignExpr {
                span: DUMMY_SP,
                op: AssignOp::Assign,
                left: AssignTarget::Simple(SimpleAssignTarget::Member(member)),
                right: value,
            });
        } else {
            *expr = Expr::Member(member);
        }
    }
}

impl PrivateFieldAccessRewriter<'_> {
    fn extract_private_helper_access(&self, call: &CallExpr) -> Option<(Atom, Option<Box<Expr>>)> {
        if call.args.iter().any(|arg| arg.spread.is_some()) {
            return None;
        }
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Ident(callee_ident) = callee.as_ref() else {
            return None;
        };
        let callee_key = binding_key(callee_ident);
        if self.get_helpers.contains(&callee_key) {
            let [ExprOrSpread { expr: receiver, .. }, ExprOrSpread { expr: map, .. }, ExprOrSpread { expr: kind, .. }] =
                call.args.as_slice()
            else {
                return None;
            };
            if !matches!(receiver.as_ref(), Expr::This(_)) || !is_private_field_kind(kind) {
                return None;
            }
            let private_name = self.private_name_for_map(map)?;
            return Some((private_name, None));
        }
        if self.set_helpers.contains(&callee_key) {
            let [ExprOrSpread { expr: receiver, .. }, ExprOrSpread { expr: map, .. }, ExprOrSpread { expr: value, .. }, ExprOrSpread { expr: kind, .. }] =
                call.args.as_slice()
            else {
                return None;
            };
            if !matches!(receiver.as_ref(), Expr::This(_)) || !is_private_field_kind(kind) {
                return None;
            }
            let private_name = self.private_name_for_map(map)?;
            return Some((private_name, Some(value.clone())));
        }
        None
    }

    fn private_name_for_map(&self, expr: &Expr) -> Option<Atom> {
        let Expr::Ident(map_ident) = expr else {
            return None;
        };
        let map_key = binding_key(map_ident);
        if !self.promoted_maps.contains(&map_key) {
            return None;
        }
        self.private_maps.get(&map_key).cloned()
    }
}

fn is_private_field_kind(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Str(str_lit)) if str_lit.value.as_str() == Some("f"))
}

fn private_member_expr(private_name: Atom) -> MemberExpr {
    MemberExpr {
        span: DUMMY_SP,
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
