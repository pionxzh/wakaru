mod common;

use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;
use wakaru_rs::facts::{
    collect_module_facts, ExportFact, ExportKind, ImportFact, ImportKind, ModuleFacts,
};
use wakaru_rs::apply_rules_until;

/// Parse source, run Stage 1+2 (up through UnEsm), then collect facts.
fn collect_facts(source: &str) -> ModuleFacts {
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
        let mut module = parser.parse_module().expect("parse failed");

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        // Run pipeline through end of Stage 2
        apply_rules_until(&mut module, unresolved_mark, "UnEsm");

        collect_module_facts(&module)
    })
}

/// Helper to build an ImportFact for assertions.
fn import(local: &str, source: &str, kind: ImportKind) -> ImportFact {
    ImportFact {
        local: local.into(),
        source: source.into(),
        kind,
    }
}

/// Helper to build an ExportFact for assertions.
fn export(exported: &str, local: Option<&str>, kind: ExportKind) -> ExportFact {
    ExportFact {
        exported: exported.into(),
        local: local.map(|s| s.into()),
        kind,
    }
}

// ── Import kind detection ──────────────────────────────────────────

#[test]
fn default_import() {
    let facts = collect_facts(r#"import x from "./mod";"#);
    assert_eq!(facts.imports, vec![
        import("x", "./mod", ImportKind::Default),
    ]);
}

#[test]
fn namespace_import() {
    let facts = collect_facts(r#"import * as ns from "./mod";"#);
    assert_eq!(facts.imports, vec![
        import("ns", "./mod", ImportKind::Namespace),
    ]);
}

#[test]
fn named_import() {
    let facts = collect_facts(r#"import { foo } from "./mod";"#);
    assert_eq!(facts.imports, vec![
        import("foo", "./mod", ImportKind::Named("foo".into())),
    ]);
}

#[test]
fn named_import_with_alias() {
    let facts = collect_facts(r#"import { foo as bar } from "./mod";"#);
    assert_eq!(facts.imports, vec![
        import("bar", "./mod", ImportKind::Named("foo".into())),
    ]);
}

#[test]
fn mixed_imports() {
    let facts = collect_facts(r#"
import def from "./a";
import * as ns from "./b";
import { x, y as z } from "./c";
"#);
    assert_eq!(facts.imports, vec![
        import("def", "./a", ImportKind::Default),
        import("ns", "./b", ImportKind::Namespace),
        import("x", "./c", ImportKind::Named("x".into())),
        import("z", "./c", ImportKind::Named("y".into())),
    ]);
}

// ── CJS → ESM conversion ──────────────────────────────────────────

#[test]
fn require_becomes_default_import() {
    let facts = collect_facts(r#"var x = require("./mod");"#);
    assert_eq!(facts.imports, vec![
        import("x", "./mod", ImportKind::Default),
    ]);
}

#[test]
fn interop_require_default_becomes_default_import() {
    let facts = collect_facts(r#"
var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault");
var _mod = _interopRequireDefault(require("./mod"));
console.log(_mod.default);
"#);
    // After Stage 2: helper unwrapped + UnEsm converts to import
    assert_eq!(facts.imports.len(), 1);
    assert_eq!(facts.imports[0].kind, ImportKind::Default);
    assert_eq!(facts.imports[0].source.as_ref(), "./mod");
}

// ── Export kind detection ──────────────────────────────────────────

#[test]
fn export_default_expr() {
    let facts = collect_facts(r#"export default 42;"#);
    assert_eq!(facts.exports, vec![
        export("default", None, ExportKind::Default),
    ]);
}

#[test]
fn export_default_function() {
    let facts = collect_facts(r#"export default function foo() {}"#);
    assert_eq!(facts.exports, vec![
        export("default", Some("foo"), ExportKind::Default),
    ]);
}

#[test]
fn export_named_function() {
    let facts = collect_facts(r#"export function foo() {}"#);
    assert_eq!(facts.exports, vec![
        export("foo", Some("foo"), ExportKind::Named),
    ]);
}

#[test]
fn export_named_const() {
    let facts = collect_facts(r#"export const a = 1, b = 2;"#);
    assert_eq!(facts.exports, vec![
        export("a", Some("a"), ExportKind::Named),
        export("b", Some("b"), ExportKind::Named),
    ]);
}

#[test]
fn export_named_class() {
    let facts = collect_facts(r#"export class Foo {}"#);
    assert_eq!(facts.exports, vec![
        export("Foo", Some("Foo"), ExportKind::Named),
    ]);
}

#[test]
fn export_specifier_list() {
    let facts = collect_facts(r#"
const a = 1;
const b = 2;
export { a, b as c };
"#);
    assert_eq!(facts.exports, vec![
        export("a", Some("a"), ExportKind::Named),
        export("c", Some("b"), ExportKind::Named),
    ]);
}

#[test]
fn export_default_via_specifier() {
    let facts = collect_facts(r#"
const a = 1;
export { a as default };
"#);
    assert_eq!(facts.exports, vec![
        export("default", Some("a"), ExportKind::Default),
    ]);
}

// ── CJS exports → ESM ──────────────────────────────────────────────

#[test]
fn module_exports_becomes_default_export() {
    let facts = collect_facts(r#"module.exports = { foo: 1 };"#);
    assert!(!facts.exports.is_empty());
    assert!(
        facts.exports.iter().any(|e| e.kind == ExportKind::Default),
        "should have a default export, got: {facts}"
    );
}

#[test]
fn exports_dot_name_becomes_named_export() {
    let facts = collect_facts(r#"
exports.foo = function() {};
exports.bar = 42;
"#);
    assert!(
        facts.exports.iter().any(|e| e.exported.as_ref() == "foo" && e.kind == ExportKind::Named),
        "should have named export 'foo', got: {facts}"
    );
    assert!(
        facts.exports.iter().any(|e| e.exported.as_ref() == "bar" && e.kind == ExportKind::Named),
        "should have named export 'bar', got: {facts}"
    );
}

// ── No imports or exports ──────────────────────────────────────────

#[test]
fn plain_code_has_empty_facts() {
    let facts = collect_facts(r#"console.log("hello");"#);
    assert!(facts.imports.is_empty());
    assert!(facts.exports.is_empty());
}

// ── Display ────────────────────────────────────────────────────────

#[test]
fn display_formatting() {
    let facts = collect_facts(r#"
import x from "./a";
import { foo as bar } from "./b";
export const val = 1;
export default 42;
"#);
    let display = format!("{facts}");
    assert!(display.contains("import x from \"./a\" [default]"), "got: {display}");
    assert!(display.contains("import bar from \"./b\" [named(foo)]"), "got: {display}");
    assert!(display.contains("export val [named]"), "got: {display}");
    assert!(display.contains("export default [default]"), "got: {display}");
}

// ── Side-effect-only import ────────────────────────────────────────

#[test]
fn side_effect_import_produces_no_bindings() {
    let facts = collect_facts(r#"import "./side-effect";"#);
    // No specifiers → no import facts
    assert!(facts.imports.is_empty());
}
