use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Span, SyntaxContext};
use swc_core::ecma::ast::{
    ArrayPat, ArrowExpr, AssignExpr, AssignOp, AssignTarget, BindingIdent, Callee, CatchClause,
    ClassDecl, ClassExpr, Expr, FnDecl, FnExpr, ForHead, ForInStmt, ForOfStmt, Function, Ident,
    ImportDecl, ImportSpecifier, JSXElementName, JSXObject, KeyValuePatProp, MemberExpr,
    MemberProp, Module, ModuleItem, ObjectPat, ObjectPatProp, Pat, PropName, SimpleAssignTarget,
    Stmt, UnaryExpr, UnaryOp, UpdateExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

pub(crate) type BindingId = (Atom, SyntaxContext);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UseKind {
    Read,
    Write,
    ReadWrite,
    CallCallee,
    NewCallee,
    StaticMemberRead(Atom),
    ComputedMemberRead,
    StaticMemberWrite(Atom),
    ComputedMemberWrite,
    DeleteTarget,
    TypeofOperand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UseSite {
    pub(crate) kind: UseKind,
    pub(crate) span: Span,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BindingInfo {
    pub(crate) declarations: Vec<Span>,
    pub(crate) uses: Vec<UseSite>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BindingUseIndex {
    bindings: HashMap<BindingId, BindingInfo>,
    uninitialized: HashSet<BindingId>,
    legacy_ident_occurrences: HashMap<BindingId, usize>,
}

impl BindingUseIndex {
    pub(crate) fn collect(module: &Module) -> Self {
        Self::collect_node(module)
    }

    pub(crate) fn collect_module_items(items: &[ModuleItem]) -> Self {
        Self::collect_node(items)
    }

    pub(crate) fn collect_stmts(stmts: &[Stmt]) -> Self {
        Self::collect_node(stmts)
    }

    fn collect_node<T>(node: &T) -> Self
    where
        T: VisitWith<BindingUseCollector> + VisitWith<LegacyIdentCounter> + ?Sized,
    {
        let mut collector = BindingUseCollector::default();
        node.visit_with(&mut collector);

        let mut legacy = LegacyIdentCounter::default();
        node.visit_with(&mut legacy);

        Self {
            bindings: collector.bindings,
            uninitialized: collector.uninitialized,
            legacy_ident_occurrences: legacy.references,
        }
    }

    pub(crate) fn uninitialized_bindings(&self) -> HashSet<BindingId> {
        self.uninitialized.clone()
    }

    /// Compatibility count for older rules that intentionally count declaration
    /// identifiers as occurrences. New consumers should prefer `use_count`.
    pub(crate) fn legacy_reference_counts(&self) -> HashMap<BindingId, usize> {
        self.legacy_ident_occurrences.clone()
    }

    pub(crate) fn referenced_bindings(&self) -> HashSet<BindingId> {
        self.bindings
            .keys()
            .filter(|binding| self.use_count(binding) > 0)
            .cloned()
            .collect()
    }

    pub(crate) fn new_callee_bindings(&self) -> HashSet<BindingId> {
        self.bindings
            .iter()
            .filter(|(_, info)| {
                info.uses
                    .iter()
                    .any(|site| matches!(site.kind, UseKind::NewCallee))
            })
            .map(|(binding, _)| binding.clone())
            .collect()
    }

    pub(crate) fn use_count(&self, binding: &BindingId) -> usize {
        self.bindings
            .get(binding)
            .map(|info| info.uses.len())
            .unwrap_or(0)
    }

    pub(crate) fn has_direct_write(&self, binding: &BindingId) -> bool {
        self.bindings.get(binding).is_some_and(|info| {
            info.uses
                .iter()
                .any(|site| matches!(site.kind, UseKind::Write | UseKind::ReadWrite))
        })
    }

    pub(crate) fn has_declaration(&self, binding: &BindingId) -> bool {
        self.bindings
            .get(binding)
            .is_some_and(|info| !info.declarations.is_empty())
    }

    #[cfg(test)]
    fn use_kinds(&self, binding: &BindingId) -> Vec<UseKind> {
        self.bindings
            .get(binding)
            .map(|info| info.uses.iter().map(|site| site.kind.clone()).collect())
            .unwrap_or_default()
    }
}

#[derive(Default)]
struct BindingUseCollector {
    bindings: HashMap<BindingId, BindingInfo>,
    uninitialized: HashSet<BindingId>,
}

impl BindingUseCollector {
    fn binding_id(ident: &Ident) -> BindingId {
        (ident.sym.clone(), ident.ctxt)
    }

    fn binding_ident_id(binding: &BindingIdent) -> BindingId {
        (binding.id.sym.clone(), binding.id.ctxt)
    }

    fn record_decl(&mut self, binding: &BindingIdent) {
        let id = Self::binding_ident_id(binding);
        self.bindings
            .entry(id)
            .or_default()
            .declarations
            .push(binding.id.span);
    }

    fn record_ident_decl(&mut self, ident: &Ident) {
        let id = Self::binding_id(ident);
        self.bindings
            .entry(id)
            .or_default()
            .declarations
            .push(ident.span);
    }

    fn record_use(&mut self, ident: &Ident, kind: UseKind) {
        let id = Self::binding_id(ident);
        self.bindings.entry(id).or_default().uses.push(UseSite {
            kind,
            span: ident.span,
        });
    }

    fn record_member_use(&mut self, member: &MemberExpr, write: bool) {
        let kind = match &member.prop {
            MemberProp::Ident(prop) if write => UseKind::StaticMemberWrite(prop.sym.clone()),
            MemberProp::Ident(prop) => UseKind::StaticMemberRead(prop.sym.clone()),
            MemberProp::Computed(_) if write => UseKind::ComputedMemberWrite,
            MemberProp::Computed(_) => UseKind::ComputedMemberRead,
            MemberProp::PrivateName(_) if write => UseKind::ComputedMemberWrite,
            MemberProp::PrivateName(_) => UseKind::ComputedMemberRead,
        };

        if let Expr::Ident(obj) = member.obj.as_ref() {
            self.record_use(obj, kind);
        } else {
            member.obj.visit_with(self);
        }

        if let MemberProp::Computed(computed) = &member.prop {
            computed.expr.visit_with(self);
        }
    }

    fn record_pat_decls(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(binding) => self.record_decl(binding),
            Pat::Array(ArrayPat { elems, .. }) => {
                for elem in elems.iter().flatten() {
                    self.record_pat_decls(elem);
                }
            }
            Pat::Object(ObjectPat { props, .. }) => {
                for prop in props {
                    match prop {
                        ObjectPatProp::KeyValue(KeyValuePatProp { key, value }) => {
                            self.visit_prop_name_exprs(key);
                            self.record_pat_decls(value);
                        }
                        ObjectPatProp::Assign(assign) => {
                            self.record_ident_decl(&assign.key);
                            if let Some(value) = &assign.value {
                                value.visit_with(self);
                            }
                        }
                        ObjectPatProp::Rest(rest) => self.record_pat_decls(&rest.arg),
                    }
                }
            }
            Pat::Assign(assign) => {
                self.record_pat_decls(&assign.left);
                assign.right.visit_with(self);
            }
            Pat::Rest(rest) => self.record_pat_decls(&rest.arg),
            Pat::Expr(expr) => expr.visit_with(self),
            Pat::Invalid(_) => {}
        }
    }

    fn record_assignment_target(&mut self, target: &AssignTarget, kind: UseKind) {
        match target {
            AssignTarget::Simple(simple) => self.record_simple_assignment_target(simple, kind),
            AssignTarget::Pat(pat) => self.visit_assignment_pat(pat, kind),
        }
    }

    fn record_simple_assignment_target(&mut self, target: &SimpleAssignTarget, kind: UseKind) {
        match target {
            SimpleAssignTarget::Ident(binding) => self.record_use(&binding.id, kind),
            SimpleAssignTarget::Member(member) => self.record_member_use(member, true),
            SimpleAssignTarget::Paren(paren) => self.record_assignment_expr(&paren.expr, kind),
            SimpleAssignTarget::TsAs(ts_as) => {
                self.record_assignment_expr(&ts_as.expr, kind);
                ts_as.type_ann.visit_with(self);
            }
            SimpleAssignTarget::TsSatisfies(ts_satisfies) => {
                self.record_assignment_expr(&ts_satisfies.expr, kind);
                ts_satisfies.type_ann.visit_with(self);
            }
            SimpleAssignTarget::TsNonNull(ts_non_null) => {
                self.record_assignment_expr(&ts_non_null.expr, kind);
            }
            SimpleAssignTarget::TsTypeAssertion(ts_assertion) => {
                self.record_assignment_expr(&ts_assertion.expr, kind);
                ts_assertion.type_ann.visit_with(self);
            }
            SimpleAssignTarget::TsInstantiation(ts_instantiation) => {
                self.record_assignment_expr(&ts_instantiation.expr, kind);
                ts_instantiation.type_args.visit_with(self);
            }
            _ => target.visit_children_with(self),
        }
    }

    fn record_assignment_expr(&mut self, expr: &Expr, kind: UseKind) {
        match expr {
            Expr::Ident(ident) => self.record_use(ident, kind),
            Expr::Member(member) => self.record_member_use(member, true),
            Expr::Paren(paren) => self.record_assignment_expr(&paren.expr, kind),
            Expr::TsAs(ts_as) => {
                self.record_assignment_expr(&ts_as.expr, kind);
                ts_as.type_ann.visit_with(self);
            }
            Expr::TsSatisfies(ts_satisfies) => {
                self.record_assignment_expr(&ts_satisfies.expr, kind);
                ts_satisfies.type_ann.visit_with(self);
            }
            Expr::TsNonNull(ts_non_null) => self.record_assignment_expr(&ts_non_null.expr, kind),
            Expr::TsTypeAssertion(ts_assertion) => {
                self.record_assignment_expr(&ts_assertion.expr, kind);
                ts_assertion.type_ann.visit_with(self);
            }
            Expr::TsInstantiation(ts_instantiation) => {
                self.record_assignment_expr(&ts_instantiation.expr, kind);
                ts_instantiation.type_args.visit_with(self);
            }
            _ => expr.visit_with(self),
        }
    }

    fn visit_assignment_pat(&mut self, pat: &swc_core::ecma::ast::AssignTargetPat, kind: UseKind) {
        match pat {
            swc_core::ecma::ast::AssignTargetPat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.record_pat_decls_as_writes(elem, kind.clone());
                }
            }
            swc_core::ecma::ast::AssignTargetPat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                            self.visit_prop_name_exprs(&kv.key);
                            self.visit_assignment_pat_or_expr(&kv.value, kind.clone());
                        }
                        swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                            self.record_use(&assign.key, kind.clone());
                            if let Some(value) = &assign.value {
                                value.visit_with(self);
                            }
                        }
                        swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                            self.visit_assignment_pat_or_expr(&rest.arg, kind.clone());
                        }
                    }
                }
            }
            swc_core::ecma::ast::AssignTargetPat::Invalid(_) => {}
        }
    }

    fn visit_assignment_pat_or_expr(&mut self, pat: &Pat, kind: UseKind) {
        match pat {
            Pat::Ident(binding) => self.record_use(&binding.id, kind),
            Pat::Array(_) | Pat::Object(_) | Pat::Assign(_) | Pat::Rest(_) => {
                self.record_pat_decls_as_writes(pat, kind)
            }
            Pat::Expr(expr) => self.record_assignment_expr(expr, kind),
            Pat::Invalid(_) => {}
        }
    }

    fn record_pat_decls_as_writes(&mut self, pat: &Pat, kind: UseKind) {
        match pat {
            Pat::Ident(binding) => self.record_use(&binding.id, kind),
            Pat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.record_pat_decls_as_writes(elem, kind.clone());
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        ObjectPatProp::KeyValue(kv) => {
                            self.visit_prop_name_exprs(&kv.key);
                            self.record_pat_decls_as_writes(&kv.value, kind.clone());
                        }
                        ObjectPatProp::Assign(assign) => {
                            self.record_use(&assign.key, kind.clone());
                            if let Some(value) = &assign.value {
                                value.visit_with(self);
                            }
                        }
                        ObjectPatProp::Rest(rest) => {
                            self.record_pat_decls_as_writes(&rest.arg, kind.clone());
                        }
                    }
                }
            }
            Pat::Assign(assign) => {
                self.record_pat_decls_as_writes(&assign.left, kind);
                assign.right.visit_with(self);
            }
            Pat::Rest(rest) => self.record_pat_decls_as_writes(&rest.arg, kind),
            Pat::Expr(expr) => self.record_assignment_expr(expr, kind),
            Pat::Invalid(_) => {}
        }
    }

    fn visit_for_head_as_assignment(&mut self, head: &ForHead) {
        match head {
            ForHead::Pat(pat) => self.record_pat_decls_as_writes(pat, UseKind::Write),
            _ => head.visit_with(self),
        }
    }

    fn visit_function_params_and_body(&mut self, function: &Function) {
        for param in &function.params {
            self.record_pat_decls(&param.pat);
        }
        function.decorators.visit_with(self);
        function.return_type.visit_with(self);
        function.type_params.visit_with(self);
        if let Some(body) = &function.body {
            body.visit_with(self);
        }
    }

    fn visit_prop_name_exprs(&mut self, prop: &PropName) {
        if let PropName::Computed(computed) = prop {
            computed.expr.visit_with(self);
        }
    }

    fn visit_jsx_member_expr_name(&mut self, member: &swc_core::ecma::ast::JSXMemberExpr) {
        match &member.obj {
            JSXObject::Ident(ident) => self.record_use(ident, UseKind::Read),
            JSXObject::JSXMemberExpr(member) => self.visit_jsx_member_expr_name(member),
        }
    }
}

impl Visit for BindingUseCollector {
    fn visit_import_decl(&mut self, import: &ImportDecl) {
        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Default(default) => self.record_ident_decl(&default.local),
                ImportSpecifier::Namespace(namespace) => self.record_ident_decl(&namespace.local),
                ImportSpecifier::Named(named) => self.record_ident_decl(&named.local),
            }
        }
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if declarator.init.is_none() {
            if let Pat::Ident(binding) = &declarator.name {
                self.uninitialized.insert(Self::binding_ident_id(binding));
            }
        }
        self.record_pat_decls(&declarator.name);
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.record_ident_decl(&decl.ident);
        self.visit_function_params_and_body(&decl.function);
    }

    fn visit_fn_expr(&mut self, expr: &FnExpr) {
        if let Some(ident) = &expr.ident {
            self.record_ident_decl(ident);
        }
        self.visit_function_params_and_body(&expr.function);
    }

    fn visit_function(&mut self, function: &Function) {
        self.visit_function_params_and_body(function);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        for param in &arrow.params {
            self.record_pat_decls(param);
        }
        arrow.body.visit_with(self);
        arrow.return_type.visit_with(self);
        arrow.type_params.visit_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        self.record_ident_decl(&decl.ident);
        decl.class.visit_with(self);
    }

    fn visit_class_expr(&mut self, expr: &ClassExpr) {
        if let Some(ident) = &expr.ident {
            self.record_ident_decl(ident);
        }
        expr.class.visit_with(self);
    }

    fn visit_catch_clause(&mut self, catch: &CatchClause) {
        if let Some(param) = &catch.param {
            self.record_pat_decls(param);
        }
        catch.body.visit_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        let kind = if assign.op == AssignOp::Assign {
            UseKind::Write
        } else {
            UseKind::ReadWrite
        };
        self.record_assignment_target(&assign.left, kind);
        assign.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        self.record_assignment_expr(&update.arg, UseKind::ReadWrite);
    }

    fn visit_for_in_stmt(&mut self, for_in: &ForInStmt) {
        // A non-declaration loop head is assigned on every iteration. Default
        // AST traversal sees its identifiers as reads, which is insufficient
        // for consumers deciding whether a binding can be declared `const`.
        self.visit_for_head_as_assignment(&for_in.left);
        for_in.right.visit_with(self);
        for_in.body.visit_with(self);
    }

    fn visit_for_of_stmt(&mut self, for_of: &ForOfStmt) {
        self.visit_for_head_as_assignment(&for_of.left);
        for_of.right.visit_with(self);
        for_of.body.visit_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        match unary.op {
            UnaryOp::Delete => {
                if let Expr::Member(member) = unary.arg.as_ref() {
                    if let Expr::Ident(obj) = member.obj.as_ref() {
                        self.record_use(obj, UseKind::DeleteTarget);
                    } else {
                        member.obj.visit_with(self);
                    }
                    if let MemberProp::Computed(computed) = &member.prop {
                        computed.expr.visit_with(self);
                    }
                } else {
                    unary.arg.visit_with(self);
                }
            }
            UnaryOp::TypeOf => {
                if let Expr::Ident(ident) = unary.arg.as_ref() {
                    self.record_use(ident, UseKind::TypeofOperand);
                } else {
                    unary.arg.visit_with(self);
                }
            }
            _ => unary.arg.visit_with(self),
        }
    }

    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        match &call.callee {
            Callee::Expr(callee) => match callee.as_ref() {
                Expr::Ident(ident) => self.record_use(ident, UseKind::CallCallee),
                Expr::Member(member) => self.record_member_use(member, false),
                _ => callee.visit_with(self),
            },
            _ => call.callee.visit_with(self),
        }
        call.args.visit_with(self);
        call.type_args.visit_with(self);
    }

    fn visit_new_expr(&mut self, new_expr: &swc_core::ecma::ast::NewExpr) {
        match new_expr.callee.as_ref() {
            Expr::Ident(ident) => self.record_use(ident, UseKind::NewCallee),
            _ => new_expr.callee.visit_with(self),
        }
        new_expr.args.visit_with(self);
        new_expr.type_args.visit_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        self.record_member_use(member, false);
    }

    fn visit_jsx_element_name(&mut self, name: &JSXElementName) {
        match name {
            JSXElementName::Ident(ident) => self.record_use(ident, UseKind::Read),
            JSXElementName::JSXMemberExpr(member) => self.visit_jsx_member_expr_name(member),
            JSXElementName::JSXNamespacedName(_) => {}
        }
    }

    fn visit_ident(&mut self, ident: &Ident) {
        self.record_use(ident, UseKind::Read);
    }

    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        self.record_decl(binding);
    }

    fn visit_prop_name(&mut self, prop: &PropName) {
        self.visit_prop_name_exprs(prop);
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            computed.expr.visit_with(self);
        }
    }
}

#[derive(Default)]
struct LegacyIdentCounter {
    references: HashMap<BindingId, usize>,
}

impl Visit for LegacyIdentCounter {
    fn visit_ident(&mut self, ident: &Ident) {
        *self
            .references
            .entry((ident.sym.clone(), ident.ctxt))
            .or_insert(0) += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{sync::Lrc, FileName, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    fn parse(source: &str) -> Module {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("test.js".into()).into(),
            source.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax {
                jsx: true,
                ..Default::default()
            }),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        Parser::new_from(lexer)
            .parse_module()
            .expect("source should parse")
    }

    fn resolved(source: &str) -> Module {
        GLOBALS.set(&Default::default(), || {
            let mut module = parse(source);
            module.visit_mut_with(&mut resolver(Default::default(), Default::default(), false));
            module
        })
    }

    fn binding(module: &Module, name: &str) -> BindingId {
        let index = BindingUseIndex::collect(module);
        index
            .bindings
            .keys()
            .find(|(sym, _)| sym.as_ref() == name)
            .cloned()
            .expect("binding should exist")
    }

    #[test]
    fn separates_declarations_from_references() {
        let module = resolved("import foo from 'x'; const bar = foo; foo.bar();");
        let index = BindingUseIndex::collect(&module);
        let foo = binding(&module, "foo");
        let bar = binding(&module, "bar");

        assert_eq!(index.use_count(&foo), 2);
        assert_eq!(index.use_count(&bar), 0);
        assert!(index.referenced_bindings().contains(&foo));
        assert!(!index.referenced_bindings().contains(&bar));
    }

    #[test]
    fn classifies_member_and_write_uses() {
        let module = resolved("let obj; obj.value; obj.value = next; delete obj.value;");
        let index = BindingUseIndex::collect(&module);
        let obj = binding(&module, "obj");

        assert_eq!(
            index.use_kinds(&obj),
            vec![
                UseKind::StaticMemberRead("value".into()),
                UseKind::StaticMemberWrite("value".into()),
                UseKind::DeleteTarget,
            ]
        );
        assert!(!index.has_direct_write(&obj));
    }

    #[test]
    fn classifies_parenthesized_assignment_and_update_targets() {
        let module = resolved("let tmp; (tmp) = value; (tmp) += next; (tmp)++;");
        let index = BindingUseIndex::collect(&module);
        let tmp = binding(&module, "tmp");

        assert_eq!(
            index.use_kinds(&tmp),
            vec![UseKind::Write, UseKind::ReadWrite, UseKind::ReadWrite]
        );
        assert!(index.has_direct_write(&tmp));
    }

    #[test]
    fn classifies_for_in_and_for_of_assignment_heads_as_writes() {
        let module = resolved(
            "let key, value, nested, parenthesized; for (key in object) {} for (value of values) {} for ({ nested } of items) {} for ((parenthesized) of items) {}",
        );
        let index = BindingUseIndex::collect(&module);

        for name in ["key", "value", "nested", "parenthesized"] {
            let id = binding(&module, name);
            assert_eq!(index.use_kinds(&id), vec![UseKind::Write]);
            assert!(index.has_direct_write(&id));
        }
    }

    #[test]
    fn write_position_inventory_classifies_every_direct_write() {
        // One row per syntactic position that writes an existing binding.
        // New write positions belong in this table first — gaps here have
        // shipped as rule bugs (for-of heads, parenthesized targets,
        // wrapped update expressions).
        let write_positions = [
            "x = 1;",
            "x += 1;",
            "x ||= 1;",
            "x &&= 1;",
            "x ??= 1;",
            "x++;",
            "--x;",
            "(x) = 1;",
            "(x)++;",
            "((x)) += 1;",
            "[x] = arr;",
            "[, x] = arr;",
            "[...x] = arr;",
            "[x = 1] = arr;",
            "[[x]] = arr;",
            "({ x } = obj);",
            "({ p: x } = obj);",
            "({ x = 1 } = obj);",
            "({ ...x } = obj);",
            "({ p: [x] } = obj);",
            "for (x of xs) {}",
            "for ((x) of xs) {}",
            "for ([x] of xs) {}",
            "for ({ p: x } of xs) {}",
            "for (x in obj) {}",
            "for ((x) in obj) {}",
            "async function f() { for await (x of xs) {} }",
            "function g() { x = 1; }",
        ];
        for stmt in write_positions {
            let module = resolved(&format!("let x; {stmt}"));
            let index = BindingUseIndex::collect(&module);
            let x = binding(&module, "x");
            assert!(
                index.has_direct_write(&x),
                "`{stmt}` must classify `x` as a direct write"
            );
        }

        // Boundary: uses that must NOT count as direct binding writes.
        let non_writes = [
            "use(x);",
            "x.p = 1;",
            "x[k] = 1;",
            "typeof x;",
            "delete x.p;",
            "const y = x;",
        ];
        for stmt in non_writes {
            let module = resolved(&format!("let x; {stmt}"));
            let index = BindingUseIndex::collect(&module);
            let x = binding(&module, "x");
            assert!(
                !index.has_direct_write(&x),
                "`{stmt}` must not classify `x` as a direct write"
            );
        }
    }

    #[test]
    fn exposes_direct_new_callee_bindings() {
        let module = resolved("let C, ns; new C(); new ns.C(); C();");
        let index = BindingUseIndex::collect(&module);
        let c = binding(&module, "C");
        let ns = binding(&module, "ns");
        let new_callees = index.new_callee_bindings();

        assert!(new_callees.contains(&c));
        assert!(!new_callees.contains(&ns));
    }

    #[test]
    fn collects_module_items_without_counting_import_specifier_bindings_as_uses() {
        let module = resolved("import createElement from 'react'; const view = createElement;");
        let index = BindingUseIndex::collect_module_items(&module.body);
        let create_element = binding(&module, "createElement");

        assert_eq!(index.use_count(&create_element), 1);
        assert!(index.referenced_bindings().contains(&create_element));
    }

    #[test]
    fn keeps_legacy_occurrence_count_for_existing_temp_rules() {
        let module = resolved("var tmp; tmp = value; consume(tmp);");
        let index = BindingUseIndex::collect(&module);
        let tmp = binding(&module, "tmp");

        assert_eq!(index.use_count(&tmp), 2);
        assert_eq!(index.legacy_reference_counts().get(&tmp).copied(), Some(3));
    }
}
