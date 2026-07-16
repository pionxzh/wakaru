//! Tests for the `MergeDeclarationInit` rule: fold a bare `let x;` / `var x;`
//! declaration into its first later assignment in the same statement list.

use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;
use wakaru_core::{rules::MergeDeclarationInit, RewriteLevel};

fn apply(src: &str) -> String {
    apply_with_level(src, RewriteLevel::Aggressive)
}

fn apply_with_level(src: &str, level: RewriteLevel) -> String {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(swc_core::common::FileName::Anon.into(), src.to_string());
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax::default()),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        let mut module: Module = Parser::new_from(lexer).parse_module().expect("parse");
        let unresolved = Mark::new();
        let top = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved, top, false));
        module.visit_mut_with(&mut MergeDeclarationInit::new(level));
        print(&module, cm)
    })
}

fn print(module: &Module, cm: Lrc<SourceMap>) -> String {
    let mut buf = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default(),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm, "\n", &mut buf, None),
        };
        emitter.emit_module(module).expect("emit");
    }
    String::from_utf8(buf)
        .unwrap()
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn merges_adjacent_let_and_assignment() {
    let out = apply("function f(){ let x; x = g(); return x; }");
    assert!(out.contains("let x = g();"), "got: {out}");
    assert!(!out.contains("let x;"), "bare decl should be gone: {out}");
}

#[test]
fn standard_merges_inert_adjacent_object_literal() {
    let out = apply_with_level(
        "function f(){ let x; x = {}; x.ready = true; return x; }",
        RewriteLevel::Standard,
    );
    assert!(out.contains("let x = {};"), "got: {out}");
}

#[test]
fn standard_merges_top_level_literal_object_as_const() {
    let out = apply_with_level(
        "let Mode; Mode = { Dev: 0, 0: 'Dev' }; use(Mode);",
        RewriteLevel::Standard,
    );
    assert!(out.contains("const Mode = {"), "got: {out}");
    assert!(
        !out.contains("let Mode;"),
        "bare decl should be gone: {out}"
    );
}

#[test]
fn standard_merges_exported_top_level_literal_object_as_const() {
    let out = apply_with_level(
        "export let Mode; Mode = { Dev: 0, 0: 'Dev' };",
        RewriteLevel::Standard,
    );
    assert!(out.contains("export const Mode = {"), "got: {out}");
    assert!(!out.contains("export let Mode;"), "got: {out}");
}

#[test]
fn top_level_merge_stays_let_when_later_reassigned() {
    let out = apply_with_level(
        "let Mode; Mode = { Dev: 0 }; Mode = other; use(Mode);",
        RewriteLevel::Standard,
    );
    assert!(out.contains("let Mode = {"), "got: {out}");
    assert!(out.contains("Mode = other;"), "got: {out}");
    assert!(
        !out.contains("const Mode"),
        "later write forbids const: {out}"
    );
}

#[test]
fn top_level_merge_stays_let_when_direct_eval_can_reassign() {
    let out = apply_with_level(
        "let Mode; Mode = { Dev: 0 }; eval('Mode = other'); use(Mode);",
        RewriteLevel::Standard,
    );
    assert!(out.contains("let Mode = {"), "got: {out}");
    assert!(
        !out.contains("const Mode"),
        "direct eval forbids const: {out}"
    );
}

#[test]
fn top_level_merge_stays_let_when_unknown_direct_eval_can_reassign() {
    let out = apply_with_level(
        "let Mode; Mode = { Dev: 0 }; eval(source); use(Mode);",
        RewriteLevel::Standard,
    );
    assert!(out.contains("let Mode = {"), "got: {out}");
    assert!(
        !out.contains("const Mode"),
        "unknown direct eval forbids const: {out}"
    );
}

#[test]
fn unrelated_known_direct_eval_allows_const() {
    let out = apply_with_level(
        "let Mode; Mode = { Dev: 0 }; eval('other = value'); use(Mode);",
        RewriteLevel::Standard,
    );
    assert!(out.contains("const Mode = {"), "got: {out}");
}

#[test]
fn top_level_merge_stays_let_when_nested_function_reassigns() {
    let out = apply_with_level(
        "let Mode; Mode = { Dev: 0 }; function replace(){ Mode = other; } use(Mode);",
        RewriteLevel::Standard,
    );
    assert!(out.contains("let Mode = {"), "got: {out}");
    assert!(
        !out.contains("const Mode"),
        "nested write forbids const: {out}"
    );
}

#[test]
fn standard_preserves_top_level_non_literal_object_assignment() {
    let input = "let Mode; Mode = { Dev: makeMode() }; use(Mode);";
    let out = apply_with_level(input, RewriteLevel::Standard);
    assert!(out.contains("let Mode;"), "got: {out}");
    assert!(out.contains("Mode = {"), "got: {out}");
}

#[test]
fn standard_preserves_top_level_observable_computed_key() {
    let input = "let Mode; Mode = { [{}]: 0 }; use(Mode);";
    let out = apply_with_level(input, RewriteLevel::Standard);
    assert!(out.contains("let Mode;"), "got: {out}");
    assert!(out.contains("Mode = {"), "got: {out}");
}

#[test]
fn standard_does_not_merge_observable_call_rhs() {
    let out = apply_with_level(
        "function f(){ let x; x = g(); function g(){ return x; } return x; }",
        RewriteLevel::Standard,
    );
    assert!(out.contains("let x;"), "bare decl must stay: {out}");
    assert!(out.contains("x = g();"), "assignment must stay: {out}");
}

#[test]
fn merges_hoisted_declarations() {
    let out = apply("function f(){ let a; let b; a = p(); b = q(a); return b; }");
    assert!(out.contains("let a = p();"), "got: {out}");
    assert!(!out.contains("let a;"), "got: {out}");
    assert!(
        out.contains("let b;"),
        "b must stay hoisted across a's initializer: {out}"
    );
    assert!(out.contains("b = q(a);"), "got: {out}");
}

#[test]
fn merges_var_declarations() {
    let out = apply("function f(){ var x; x = 1; return x; }");
    assert!(out.contains("var x = 1;"), "got: {out}");
}

#[test]
fn does_not_merge_when_referenced_between() {
    // x is read before its assignment; moving the init would change behavior.
    let out = apply("function f(){ let x; sink(x); x = 1; return x; }");
    assert!(out.contains("let x;"), "bare decl must stay: {out}");
    assert!(!out.contains("let x = 1"), "got: {out}");
}

#[test]
fn does_not_merge_when_rhs_references_self() {
    let out = apply("function f(){ let x; x = x + 1; return x; }");
    assert!(out.contains("let x;"), "bare decl must stay: {out}");
}

#[test]
fn does_not_merge_assignment_in_nested_block() {
    // Assignment is not in the same statement list as the declaration.
    let out = apply("function f(){ let x; if (c) { x = 1; } return x; }");
    assert!(out.contains("let x;"), "bare decl must stay: {out}");
}

#[test]
fn does_not_touch_compound_assignment() {
    let out = apply("function f(){ let x; x += 1; return x; }");
    assert!(out.contains("let x;"), "bare decl must stay: {out}");
}

#[test]
fn leaves_later_reassignments_as_assignments() {
    let out = apply("function f(){ let x; x = 1; x = 2; return x; }");
    assert!(out.contains("let x = 1;"), "got: {out}");
    assert!(out.contains("x = 2;"), "second assignment stays: {out}");
}

#[test]
fn does_not_merge_when_closure_captures_binding_between() {
    // The nested function writes the outer `x` between the declaration and its
    // first statement-level assignment. Moving the `let` past the closure would
    // put `x` in the TDZ if the closure runs first, so the merge is skipped.
    let out = apply("function f(){ let x; function g(){ x = 1; } x = top(); return x; }");
    assert!(out.contains("let x;"), "bare decl must stay: {out}");
}

#[test]
fn does_not_merge_across_intervening_call() {
    // `g()` may read the hoisted declaration through a closure. Moving `let x`
    // below the call would change an initialized-to-undefined read into TDZ.
    let out = apply("function f(){ let x; g(); x = 1; function g(){ return x; } return x; }");
    assert!(out.contains("let x;"), "bare decl must stay: {out}");
    assert!(
        !out.contains("let x = 1;"),
        "init must not move past call: {out}"
    );
}

#[test]
fn inner_scope_binding_does_not_block_outer_merge() {
    // The inner `y` is a *separate* binding; it must not be treated as a
    // reference to the outer one. The intervening function declaration still
    // keeps the outer declaration in place because it can observe declaration
    // timing if called before the assignment.
    let out = apply(
        "function f(){ let x; function g(){ let x = 1; return x; } x = top(); return g() + x; }",
    );
    assert!(
        out.contains("let x;"),
        "outer declaration should stay hoisted: {out}"
    );
    assert!(out.contains("x = top();"), "assignment should stay: {out}");
}
