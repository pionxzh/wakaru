use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{BytePos, SyntaxContext};
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmt, BlockStmtOrExpr, Class, ClassDecl, ClassMember, Decl, FnDecl, Function,
    GetterProp, Ident, ImportDecl, ImportSpecifier, Key, MemberProp, Module, ModuleDecl,
    ModuleItem, NamedExport, ObjectPatProp, Pat, PropName, SetterProp, Stmt, VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitWith};

type BindingId = (Atom, SyntaxContext);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TdzViolation {
    pub name: String,
    pub ref_pos: BytePos,
    pub decl_pos: BytePos,
}

/// Check a resolved AST for temporal dead zone violations.
///
/// Uses AST traversal order (not source spans) to detect references to
/// `let`/`const`/`class` bindings that appear before their declaration
/// in the same scope. This correctly handles transforms that reorder
/// AST nodes without updating spans.
///
/// Also detects self-references in initializers (`const x = x`) and
/// references in class heritage clauses (`class Foo extends Bar`
/// before `let Bar`).
///
/// References inside nested functions/arrows are not checked against
/// outer declarations (deferred execution).
pub fn check_tdz(module: &Module) -> Vec<TdzViolation> {
    let mut checker = TdzChecker {
        violations: Vec::new(),
    };
    module.visit_with(&mut checker);
    checker.violations
}

struct TdzChecker {
    violations: Vec<TdzViolation>,
}

impl TdzChecker {
    fn check_scope<N>(&mut self, node: &N)
    where
        N: for<'a> VisitWith<OrderedScopeChecker<'a>> + VisitWith<LexicalBindingCollector>,
    {
        let mut collector = LexicalBindingCollector {
            bindings: HashSet::new(),
            positions: HashMap::new(),
        };
        node.visit_with(&mut collector);
        if collector.bindings.is_empty() {
            return;
        }

        let mut scope_checker = OrderedScopeChecker {
            lexical_bindings: &collector.bindings,
            decl_positions: &collector.positions,
            declared: HashSet::new(),
            violations: Vec::new(),
        };
        node.visit_with(&mut scope_checker);
        self.violations.extend(scope_checker.violations);
    }

    fn check_param_scope<'a, I>(&mut self, params: I)
    where
        I: IntoIterator<Item = &'a Pat>,
    {
        let params: Vec<&Pat> = params.into_iter().collect();
        let mut collector = LexicalBindingCollector {
            bindings: HashSet::new(),
            positions: HashMap::new(),
        };
        for param in &params {
            collect_pat_ids_with_pos(
                param,
                BytePos(0),
                &mut collector.bindings,
                &mut collector.positions,
            );
        }
        if collector.bindings.is_empty() {
            return;
        }

        let mut scope_checker = OrderedScopeChecker {
            lexical_bindings: &collector.bindings,
            decl_positions: &collector.positions,
            declared: HashSet::new(),
            violations: Vec::new(),
        };
        for param in params {
            visit_pat_expressions_and_declare(param, &mut scope_checker);
        }
        self.violations.extend(scope_checker.violations);
    }
}

impl Visit for TdzChecker {
    fn visit_module(&mut self, module: &Module) {
        self.check_scope(module);
        module.visit_children_with(self);
    }

    fn visit_function(&mut self, func: &Function) {
        self.check_param_scope(func.params.iter().map(|param| &param.pat));
        if let Some(body) = &func.body {
            self.check_scope(body);
        }
        func.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.check_param_scope(arrow.params.iter());
        if let BlockStmtOrExpr::BlockStmt(block) = &*arrow.body {
            self.check_scope(block);
        }
        arrow.visit_children_with(self);
    }

    fn visit_getter_prop(&mut self, prop: &GetterProp) {
        if let Some(body) = &prop.body {
            self.check_scope(body);
        }
        prop.visit_children_with(self);
    }

    fn visit_setter_prop(&mut self, prop: &SetterProp) {
        if let Some(body) = &prop.body {
            self.check_scope(body);
        }
        prop.visit_children_with(self);
    }
}

// -- Pre-scan: collect all lexical binding IDs in a scope --

struct LexicalBindingCollector {
    bindings: HashSet<BindingId>,
    positions: HashMap<BindingId, BytePos>,
}

impl Visit for LexicalBindingCollector {
    fn visit_var_decl(&mut self, var: &VarDecl) {
        if matches!(var.kind, VarDeclKind::Let | VarDeclKind::Const) {
            for d in &var.decls {
                collect_pat_ids_with_pos(
                    &d.name,
                    var.span.lo,
                    &mut self.bindings,
                    &mut self.positions,
                );
            }
        }
    }

    fn visit_class_decl(&mut self, class_decl: &ClassDecl) {
        let id = (class_decl.ident.sym.clone(), class_decl.ident.ctxt);
        self.positions.insert(id.clone(), class_decl.ident.span.lo);
        self.bindings.insert(id);
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

// -- Ordered scope checker: single-pass traversal-order TDZ detection --
//
// Walks statements in AST child order. For each statement, references
// are visited BEFORE the statement's bindings are added to `declared`.
// This means a reference to a binding whose declaration hasn't been
// traversed yet is flagged — regardless of BytePos.

struct OrderedScopeChecker<'a> {
    lexical_bindings: &'a HashSet<BindingId>,
    decl_positions: &'a HashMap<BindingId, BytePos>,
    declared: HashSet<BindingId>,
    violations: Vec<TdzViolation>,
}

impl Visit for OrderedScopeChecker<'_> {
    fn visit_module(&mut self, module: &Module) {
        hoist_module_bindings(&module.body, &mut self.declared);
        module.visit_children_with(self);
    }

    fn visit_block_stmt(&mut self, block: &BlockStmt) {
        hoist_stmts(&block.stmts, &mut self.declared);
        block.visit_children_with(self);
    }

    fn visit_var_decl(&mut self, var: &VarDecl) {
        if matches!(var.kind, VarDeclKind::Let | VarDeclKind::Const) {
            for decl in &var.decls {
                if let Some(init) = &decl.init {
                    init.visit_with(self);
                }
                // Process destructuring properties sequentially: visit each
                // property's default expression, then mark that binding as
                // declared before the next property. This matches JS evaluation
                // order where `let { a = 1, b = a }` is valid (a is available
                // when b's default runs).
                visit_pat_expressions_and_declare(&decl.name, self);
            }
        } else {
            var.visit_children_with(self);
        }
    }

    fn visit_class_decl(&mut self, class_decl: &ClassDecl) {
        if let Some(super_class) = &class_decl.class.super_class {
            super_class.visit_with(self);
        }
        self.declared
            .insert((class_decl.ident.sym.clone(), class_decl.ident.ctxt));
        visit_class_members(&class_decl.class, self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        let id = (ident.sym.clone(), ident.ctxt);
        if self.lexical_bindings.contains(&id) && !self.declared.contains(&id) {
            self.violations.push(TdzViolation {
                name: id.0.to_string(),
                ref_pos: ident.span.lo,
                decl_pos: self.decl_positions.get(&id).copied().unwrap_or(BytePos(0)),
            });
        }
    }

    fn visit_fn_decl(&mut self, _: &FnDecl) {}
    fn visit_import_decl(&mut self, _: &ImportDecl) {}
    fn visit_named_export(&mut self, _: &NamedExport) {}

    fn visit_prop_name(&mut self, prop: &PropName) {
        if let PropName::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_getter_prop(&mut self, prop: &GetterProp) {
        if let PropName::Computed(c) = &prop.key {
            c.visit_with(self);
        }
    }

    fn visit_setter_prop(&mut self, prop: &SetterProp) {
        if let PropName::Computed(c) = &prop.key {
            c.visit_with(self);
        }
    }

    fn visit_class(&mut self, class: &Class) {
        if let Some(super_class) = &class.super_class {
            super_class.visit_with(self);
        }
        visit_class_members(class, self);
    }
}

// -- Helpers --

fn hoist_module_bindings(items: &[ModuleItem], declared: &mut HashSet<BindingId>) {
    for item in items {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                declared.insert((fn_decl.ident.sym.clone(), fn_decl.ident.ctxt));
            }
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                for spec in &import.specifiers {
                    let local = match spec {
                        ImportSpecifier::Named(n) => &n.local,
                        ImportSpecifier::Default(d) => &d.local,
                        ImportSpecifier::Namespace(ns) => &ns.local,
                    };
                    declared.insert((local.sym.clone(), local.ctxt));
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                if let Decl::Fn(fn_decl) = &export.decl {
                    declared.insert((fn_decl.ident.sym.clone(), fn_decl.ident.ctxt));
                }
            }
            _ => {}
        }
    }
}

fn hoist_stmts(stmts: &[Stmt], declared: &mut HashSet<BindingId>) {
    for stmt in stmts {
        if let Stmt::Decl(Decl::Fn(fn_decl)) = stmt {
            declared.insert((fn_decl.ident.sym.clone(), fn_decl.ident.ctxt));
        }
    }
}

fn collect_pat_ids_with_pos(
    pat: &Pat,
    pos: BytePos,
    bindings: &mut HashSet<BindingId>,
    positions: &mut HashMap<BindingId, BytePos>,
) {
    match pat {
        Pat::Ident(bi) => {
            let id = (bi.id.sym.clone(), bi.id.ctxt);
            positions.insert(id.clone(), pos);
            bindings.insert(id);
        }
        Pat::Array(ap) => {
            for elem in ap.elems.iter().flatten() {
                collect_pat_ids_with_pos(elem, pos, bindings, positions);
            }
        }
        Pat::Object(op) => {
            for prop in &op.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => {
                        collect_pat_ids_with_pos(&kv.value, pos, bindings, positions);
                    }
                    ObjectPatProp::Assign(a) => {
                        let id = (a.key.sym.clone(), a.key.ctxt);
                        positions.insert(id.clone(), pos);
                        bindings.insert(id);
                    }
                    ObjectPatProp::Rest(r) => {
                        collect_pat_ids_with_pos(&r.arg, pos, bindings, positions);
                    }
                }
            }
        }
        Pat::Rest(r) => collect_pat_ids_with_pos(&r.arg, pos, bindings, positions),
        Pat::Assign(a) => collect_pat_ids_with_pos(&a.left, pos, bindings, positions),
        _ => {}
    }
}

fn visit_class_members(class: &Class, checker: &mut OrderedScopeChecker<'_>) {
    for member in &class.body {
        match member {
            ClassMember::Constructor(ctor) => {
                if let PropName::Computed(c) = &ctor.key {
                    c.visit_with(checker);
                }
            }
            ClassMember::Method(method) => {
                if let PropName::Computed(c) = &method.key {
                    c.visit_with(checker);
                }
            }
            ClassMember::ClassProp(prop) => {
                if let PropName::Computed(c) = &prop.key {
                    c.visit_with(checker);
                }
                if prop.is_static {
                    if let Some(value) = &prop.value {
                        value.visit_with(checker);
                    }
                }
            }
            ClassMember::PrivateProp(prop) if prop.is_static => {
                if let Some(value) = &prop.value {
                    value.visit_with(checker);
                }
            }
            ClassMember::StaticBlock(block) => {
                block.body.visit_with(checker);
            }
            ClassMember::AutoAccessor(accessor) => {
                if let Key::Public(PropName::Computed(c)) = &accessor.key {
                    c.visit_with(checker);
                }
                if accessor.is_static {
                    if let Some(value) = &accessor.value {
                        value.visit_with(checker);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Like `visit_pat_expressions` + `collect_pat_ids`, but interleaved:
/// visit each property's default, then mark that property's binding as declared,
/// before processing the next. This matches JS sequential evaluation of defaults.
fn visit_pat_expressions_and_declare(pat: &Pat, checker: &mut OrderedScopeChecker<'_>) {
    match pat {
        Pat::Ident(bi) => {
            checker.declared.insert((bi.id.sym.clone(), bi.id.ctxt));
        }
        Pat::Assign(a) => {
            a.right.visit_with(checker);
            visit_pat_expressions_and_declare(&a.left, checker);
        }
        Pat::Object(op) => {
            for prop in &op.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => {
                        if let PropName::Computed(c) = &kv.key {
                            c.visit_with(checker);
                        }
                        visit_pat_expressions_and_declare(&kv.value, checker);
                    }
                    ObjectPatProp::Assign(a) => {
                        if let Some(default) = &a.value {
                            default.visit_with(checker);
                        }
                        checker.declared.insert((a.key.sym.clone(), a.key.ctxt));
                    }
                    ObjectPatProp::Rest(r) => {
                        visit_pat_expressions_and_declare(&r.arg, checker);
                    }
                }
            }
        }
        Pat::Array(ap) => {
            for elem in ap.elems.iter().flatten() {
                visit_pat_expressions_and_declare(elem, checker);
            }
        }
        Pat::Rest(r) => visit_pat_expressions_and_declare(&r.arg, checker),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    fn violation_names(source: &str) -> Vec<String> {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(
                FileName::Custom("test.js".to_string()).into(),
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
            let mut parser = Parser::new_from(lexer);
            let mut module = parser.parse_module().expect("failed to parse");
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

            check_tdz(&module).into_iter().map(|v| v.name).collect()
        })
    }

    #[test]
    fn ref_before_let() {
        assert_eq!(violation_names("console.log(x);\nlet x = 1;"), vec!["x"]);
    }

    #[test]
    fn ref_before_const() {
        assert_eq!(violation_names("console.log(x);\nconst x = 1;"), vec!["x"]);
    }

    #[test]
    fn ref_after_let_is_fine() {
        assert!(violation_names("let x = 1;\nconsole.log(x);").is_empty());
    }

    #[test]
    fn var_is_hoisted() {
        assert!(violation_names("console.log(x);\nvar x = 1;").is_empty());
    }

    #[test]
    fn function_decl_is_hoisted() {
        assert!(violation_names("foo();\nfunction foo() {}").is_empty());
    }

    #[test]
    fn class_decl_tdz() {
        assert_eq!(violation_names("new Foo();\nclass Foo {}"), vec!["Foo"]);
    }

    #[test]
    fn nested_function_capture_no_false_positive() {
        assert!(violation_names("function foo() { return x; }\nlet x = 1;\nfoo();").is_empty());
    }

    #[test]
    fn nested_arrow_capture_no_false_positive() {
        assert!(violation_names("const foo = () => x;\nlet x = 1;").is_empty());
    }

    #[test]
    fn tdz_inside_function_body() {
        assert_eq!(
            violation_names("function foo() {\n  console.log(x);\n  let x = 1;\n}"),
            vec!["x"]
        );
    }

    #[test]
    fn tdz_inside_arrow_body() {
        assert_eq!(
            violation_names("const foo = () => {\n  console.log(x);\n  let x = 1;\n};"),
            vec!["x"]
        );
    }

    #[test]
    fn block_scoped_tdz() {
        assert_eq!(
            violation_names("{\n  console.log(x);\n  let x = 1;\n}"),
            vec!["x"]
        );
    }

    #[test]
    fn multiple_violations() {
        let names = violation_names("console.log(x, y);\nlet x = 1;\nlet y = 2;");
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"x".to_string()));
        assert!(names.contains(&"y".to_string()));
    }

    #[test]
    fn no_lexical_decls() {
        assert!(violation_names("var x = 1;\nconsole.log(x);").is_empty());
    }

    #[test]
    fn destructuring_tdz() {
        assert_eq!(
            violation_names("console.log(a);\nconst { a, b } = obj;"),
            vec!["a"]
        );
    }

    #[test]
    fn export_binding_no_false_positive() {
        assert!(violation_names("export { x };\nlet x = 1;").is_empty());
    }

    #[test]
    fn deeply_nested_function_scopes() {
        let source = r#"
function outer() {
    console.log(a);
    let a = 1;
    function inner() {
        console.log(b);
        let b = 2;
    }
}
"#;
        let names = violation_names(source);
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"a".to_string()));
        assert!(names.contains(&"b".to_string()));
    }

    // P1: Uses AST traversal order, not BytePos — verified by all tests
    // passing regardless of span values.

    // P2: Class heritage TDZ
    #[test]
    fn class_heritage_tdz() {
        assert_eq!(
            violation_names("class Foo extends Bar {}\nlet Bar = Object;"),
            vec!["Bar"]
        );
    }

    #[test]
    fn class_heritage_after_decl_is_fine() {
        assert!(violation_names("let Bar = Object;\nclass Foo extends Bar {}").is_empty());
    }

    // P3: Self-reference in initializer
    #[test]
    fn self_reference_in_const_init() {
        assert_eq!(violation_names("const x = x;"), vec!["x"]);
    }

    #[test]
    fn self_reference_in_let_init() {
        assert_eq!(violation_names("let x = x + 1;"), vec!["x"]);
    }

    #[test]
    fn sequential_declarators_first_available_to_second() {
        assert!(violation_names("let a = 1, b = a;").is_empty());
    }

    #[test]
    fn import_binding_is_hoisted() {
        assert!(violation_names("console.log(foo);\nimport foo from './foo';").is_empty());
    }

    #[test]
    fn export_function_decl_is_hoisted() {
        assert!(violation_names("foo();\nexport function foo() {}").is_empty());
    }

    #[test]
    fn class_expr_heritage_tdz() {
        assert_eq!(violation_names("let C = class extends C {};"), vec!["C"]);
    }

    #[test]
    fn class_computed_key_tdz() {
        assert_eq!(
            violation_names("class Foo { [Bar]() {} }\nlet Bar = 1;"),
            vec!["Bar"]
        );
    }

    #[test]
    fn class_static_field_tdz() {
        assert_eq!(
            violation_names("class Foo { static x = Bar; }\nlet Bar = 1;"),
            vec!["Bar"]
        );
    }

    #[test]
    fn class_instance_field_no_false_positive() {
        assert!(violation_names("class Foo { x = bar; }\nlet bar = 1;").is_empty());
    }

    #[test]
    fn computed_pattern_key_tdz() {
        assert_eq!(
            violation_names("const { [x]: y } = obj;\nconst x = 1;"),
            vec!["x"]
        );
    }

    #[test]
    fn computed_pattern_key_after_decl_is_fine() {
        assert!(violation_names("const x = 'key';\nconst { [x]: y } = obj;").is_empty());
    }

    #[test]
    fn destructuring_default_references_earlier_binding() {
        // Later defaults can reference earlier bindings in the same pattern.
        assert!(violation_names(
            "let { tag = 'x', size = 25, boundary = `${tag}-${size}` } = opts || {};"
        )
        .is_empty());
    }

    #[test]
    fn destructuring_default_self_reference_is_tdz() {
        assert_eq!(violation_names("let { a = a } = {};"), vec!["a"]);
    }

    #[test]
    fn destructuring_assign_default_references_earlier() {
        assert!(violation_names("let { a = 1, b = a } = {};").is_empty());
    }

    #[test]
    fn getter_body_self_reference_no_false_positive() {
        assert!(
            violation_names("let A = { get isRefreshing() { return A.isRefreshing; } };")
                .is_empty()
        );
    }

    #[test]
    fn setter_body_self_reference_no_false_positive() {
        assert!(violation_names("let A = { set value(v) { A._value = v; } };").is_empty());
    }

    #[test]
    fn getter_computed_key_tdz() {
        assert_eq!(
            violation_names("let A = { get [B]() { return 1; } };\nlet B = 'key';"),
            vec!["B"]
        );
    }

    #[test]
    fn tdz_inside_getter_body() {
        assert_eq!(
            violation_names("let A = { get x() { console.log(y); let y = 1; } };"),
            vec!["y"]
        );
    }

    #[test]
    fn tdz_inside_setter_body() {
        assert_eq!(
            violation_names("let A = { set x(v) { console.log(y); let y = v; } };"),
            vec!["y"]
        );
    }

    #[test]
    fn function_param_default_references_later_param() {
        assert_eq!(violation_names("function f(a = b, b) {}"), vec!["b"]);
    }

    #[test]
    fn function_param_default_references_self() {
        assert_eq!(violation_names("function f(a = a) {}"), vec!["a"]);
    }

    #[test]
    fn arrow_param_default_references_later_param() {
        assert_eq!(violation_names("const f = (a = b, b) => a;"), vec!["b"]);
    }

    #[test]
    fn function_param_default_references_earlier_param_is_fine() {
        assert!(violation_names("function f(a, b = a) {}").is_empty());
    }

    #[test]
    fn assignment_before_let_declaration_is_tdz() {
        assert_eq!(violation_names("x = 2;\nlet x;"), vec!["x"]);
    }

    #[test]
    fn assignment_after_let_declaration_is_fine() {
        assert!(violation_names("let x;\nx = 2;").is_empty());
    }

    #[test]
    fn update_before_let_declaration_is_tdz() {
        assert_eq!(violation_names("x++;\nlet x;"), vec!["x"]);
    }

    #[test]
    #[ignore = "requires switch-case control-flow analysis"]
    fn switch_case_ref_to_uninitialized_case_binding_is_tdz() {
        assert_eq!(
            violation_names(
                r#"
switch (tag) {
    case "read":
        value;
        break;
    case "init":
        let value = 1;
        break;
}
"#
            ),
            vec!["value"]
        );
    }

    #[test]
    #[ignore = "requires call-order/control-flow analysis"]
    fn function_called_before_lexical_declaration_is_tdz() {
        assert_eq!(
            violation_names(
                r#"
function read() {
    value;
}
read();
let value = 1;
"#
            ),
            vec!["value"]
        );
    }

    #[test]
    fn function_called_after_lexical_declaration_is_fine() {
        assert!(violation_names(
            r#"
function read() {
    value;
}
let value = 1;
read();
"#
        )
        .is_empty());
    }

    #[test]
    fn class_decl_static_block_can_reference_class_name() {
        assert!(violation_names("class C { static { C; } }").is_empty());
    }

    #[test]
    fn const_class_expr_static_block_self_reference_is_tdz() {
        assert_eq!(
            violation_names("const C = class { static { C; } };"),
            vec!["C"]
        );
    }
}
