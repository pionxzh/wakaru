mod common;

use common::assert_eq_normalized;
use swc_core::common::GLOBALS;
use swc_core::ecma::visit::VisitMutWith;
use wakaru_rs::rules::UnParameters;

fn apply(input: &str) -> String {
    GLOBALS.set(&Default::default(), || {
        use swc_core::common::{sync::Lrc, FileName, SourceMap};
        use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
        use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};

        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("test.js".to_string()).into(),
            input.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax { jsx: true, ..Default::default() }),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let mut module = parser.parse_module().expect("parse failed");

        module.visit_mut_with(&mut UnParameters);

        let mut output = Vec::new();
        {
            let mut emitter = Emitter {
                cfg: Config::default().with_minify(false),
                cm: cm.clone(),
                comments: None,
                wr: JsWriter::new(cm, "\n", &mut output, None),
            };
            emitter.emit_module(&module).expect("emit failed");
        }
        String::from_utf8(output).expect("utf-8")
    })
}

// --- void 0 / undefined guard patterns ---

#[test]
fn void0_guard_becomes_default_param() {
    let input = r#"
function foo(a, b) {
  if (a === void 0) { a = 1; }
  if (b === void 0) b = 2;
  return a + b;
}
"#;
    let expected = r#"
function foo(a = 1, b = 2) {
  return a + b;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn void0_guard_reversed_operands() {
    let input = r#"
function foo(a, b) {
  if (void 0 === a) a = 1;
  if (void 0 === b) { b = 2; }
  return a + b;
}
"#;
    let expected = r#"
function foo(a = 1, b = 2) {
  return a + b;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn undefined_guard_becomes_default_param() {
    let input = r#"
function foo(a, b) {
  if (a === undefined) a = 1;
  if (undefined === b) b = 2;
  return a + b;
}
"#;
    let expected = r#"
function foo(a = 1, b = 2) {
  return a + b;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn void0_guard_in_arrow_function() {
    let input = r#"
const test = (a, b) => {
  if (a === void 0) a = 1;
  if (void 0 === b) b = 2;
};
"#;
    let expected = r#"
const test = (a = 1, b = 2) => {};
"#;
    assert_eq_normalized(&apply(input), expected);
}

// --- or-assignment pattern ---

#[test]
fn or_assignment_becomes_default_param() {
    // `a = a || fallback` is a classic pre-ES6 default parameter idiom
    let input = r#"
function foo(a) {
    a = a || 2;
}
"#;
    let expected = r#"
function foo(a = 2) {}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn or_assignment_multiple_params() {
    let input = r#"
function foo(a, b, c) {
    a = a || "hello";
    b = b || {};
    c = c || [];
}
"#;
    let expected = r#"
function foo(a = "hello", b = {}, c = []) {}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// --- ternary-assignment pattern ---

#[test]
fn ternary_self_check_becomes_default_param() {
    // `a = a ? a : fallback` is equivalent to `a = a || fallback` for default params
    let input = r#"
function foo(a) {
    a = a ? a : 4;
}
"#;
    let expected = r#"
function foo(a = 4) {}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// --- no-op cases ---

#[test]
fn noop_no_guard() {
    let input = r#"
function foo(a, b) {
  return a + b;
}
"#;
    let expected = r#"
function foo(a, b) {
  return a + b;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn noop_guard_for_non_param() {
    // `c` is not in the parameter list — guard should be left untouched
    let input = r#"
function foo(a, b) {
  if (c === void 0) c = 1;
  return a + b;
}
"#;
    let expected = r#"
function foo(a, b) {
  if (c === void 0) c = 1;
  return a + b;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}
