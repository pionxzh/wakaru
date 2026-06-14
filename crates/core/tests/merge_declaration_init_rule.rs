//! Tests for the `MergeDeclarationInit` rule: fold a bare `let x;` / `var x;`
//! declaration into its first later assignment in the same statement list.

use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;
use wakaru_core::rules::MergeDeclarationInit;

fn apply(src: &str) -> String {
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
        module.visit_mut_with(&mut MergeDeclarationInit);
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
fn merges_hoisted_declarations() {
    let out = apply("function f(){ let a; let b; a = p(); b = q(a); return b; }");
    assert!(out.contains("let a = p();"), "got: {out}");
    assert!(out.contains("let b = q(a);"), "got: {out}");
    assert!(!out.contains("let a;"), "got: {out}");
    assert!(!out.contains("let b;"), "got: {out}");
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
fn inner_scope_binding_does_not_block_outer_merge() {
    // The inner `y` is a *separate* binding; it must not be treated as a
    // reference to the outer one, so the outer merge still happens.
    let out = apply(
        "function f(){ let x; function g(){ let x = 1; return x; } x = top(); return g() + x; }",
    );
    assert!(
        out.contains("let x = top();"),
        "outer merge should happen: {out}"
    );
}
