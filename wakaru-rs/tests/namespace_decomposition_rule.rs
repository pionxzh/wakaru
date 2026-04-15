mod common;

use common::{assert_eq_normalized, normalize};
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::visit::VisitMutWith;
use wakaru_rs::facts::{collect_module_facts, ModuleFacts, ModuleFactsMap};
use wakaru_rs::namespace_decomposition::run_namespace_decomposition;

/// Parse ESM source, collect facts, run namespace decomposition with given
/// cross-module facts, and return the emitted code.
fn run_decomp(source: &str, facts: &ModuleFactsMap) -> String {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("test.js".to_string()).into(),
            source.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax { jsx: true, ..Default::default() }),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let mut module = parser.parse_module().expect("parse failed");

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        run_namespace_decomposition(&mut module, facts);

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
        String::from_utf8(output).expect("utf8")
    })
}

/// Collect facts from ESM source (simulates a target module).
fn facts_for(source: &str) -> ModuleFacts {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("target.js".to_string()).into(),
            source.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax::default()),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let mut module = parser.parse_module().expect("parse failed");

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        collect_module_facts(&module)
    })
}

// ── Namespace decomposition ────────────────────────────────────────

#[test]
fn decompose_default_import_to_named() {
    let target_facts = facts_for(r#"
export function createStore() {}
export function applyMiddleware() {}
"#);
    let mut facts = ModuleFactsMap::new();
    facts.insert("./module-11.js",target_facts);

    let input = r#"
import r from "./module-11.js";
const p = r.createStore(u, r.applyMiddleware(d));
"#;
    let expected = r#"
import { applyMiddleware, createStore } from "./module-11.js";
const p = createStore(u, applyMiddleware(d));
"#;
    assert_eq_normalized(&run_decomp(input, &facts), expected.trim());
}

#[test]
fn apply_left_for_un_argument_spread() {
    // After decomposition, r.fn.apply(undefined, args) becomes fn.apply(undefined, args).
    // UnArgumentSpread (Stage 3) handles Pattern 1: fn.apply(null, args) → fn(...args).
    let target_facts = facts_for(r#"
export function createStore() {}
export function applyMiddleware() {}
"#);
    let mut facts = ModuleFactsMap::new();
    facts.insert("./module-11.js", target_facts);

    let input = r#"
import r from "./module-11.js";
const p = r.createStore(u, r.applyMiddleware.apply(undefined, d));
"#;
    let expected = r#"
import { applyMiddleware, createStore } from "./module-11.js";
const p = createStore(u, applyMiddleware.apply(undefined, d));
"#;
    assert_eq_normalized(&run_decomp(input, &facts), expected.trim());
}

// ── Safety: bare binding usage prevents decomposition ──────────────

#[test]
fn bare_binding_prevents_decomposition() {
    let target_facts = facts_for(r#"export function foo() {}"#);
    let mut facts = ModuleFactsMap::new();
    facts.insert("./mod.js",target_facts);

    let input = r#"
import r from "./mod.js";
console.log(r.foo);
doSomething(r);
"#;
    let output = run_decomp(input, &facts);
    // Should NOT decompose because `r` is used bare
    assert!(normalize(&output).contains("import r from"), "should keep default import, got: {output}");
}

// ── Safety: computed access prevents decomposition ─────────────────

#[test]
fn computed_access_prevents_decomposition() {
    let target_facts = facts_for(r#"export function foo() {}"#);
    let mut facts = ModuleFactsMap::new();
    facts.insert("./mod.js",target_facts);

    let input = r#"
import r from "./mod.js";
r[someKey];
"#;
    let output = run_decomp(input, &facts);
    assert!(normalize(&output).contains("import r from"), "should keep default import, got: {output}");
}

// ── Safety: target doesn't export accessed name ────────────────────

#[test]
fn missing_export_prevents_decomposition() {
    let target_facts = facts_for(r#"export function bar() {}"#);
    let mut facts = ModuleFactsMap::new();
    facts.insert("./mod.js",target_facts);

    let input = r#"
import r from "./mod.js";
r.foo();
"#;
    let output = run_decomp(input, &facts);
    // `foo` is not exported by target, so don't decompose
    assert!(normalize(&output).contains("import r from"), "should keep default import, got: {output}");
}

// ── Safety: unknown target module ──────────────────────────────────

#[test]
fn unknown_target_prevents_decomposition() {
    let facts = ModuleFactsMap::new(); // no facts at all

    let input = r#"
import r from "./unknown.js";
r.foo();
"#;
    let output = run_decomp(input, &facts);
    assert!(normalize(&output).contains("import r from"), "should keep default import, got: {output}");
}

// ── No-op when no default imports ──────────────────────────────────

#[test]
fn named_import_untouched() {
    let target_facts = facts_for(r#"export function foo() {}"#);
    let mut facts = ModuleFactsMap::new();
    facts.insert("./mod.js",target_facts);

    let input = r#"
import { foo } from "./mod.js";
foo();
"#;
    let output = run_decomp(input, &facts);
    assert_eq_normalized(&output, input.trim());
}

// ── Multiple decompositions ────────────────────────────────────────

#[test]
fn multiple_imports_decomposed() {
    let facts_a = facts_for(r#"export function x() {} export function y() {}"#);
    let facts_b = facts_for(r#"export function z() {}"#);
    let mut facts = ModuleFactsMap::new();
    facts.insert("./a.js",facts_a);
    facts.insert("./b.js",facts_b);

    let input = r#"
import a from "./a.js";
import b from "./b.js";
a.x();
a.y();
b.z();
"#;
    let expected = r#"
import { x, y } from "./a.js";
import { z } from "./b.js";
x();
y();
z();
"#;
    assert_eq_normalized(&run_decomp(input, &facts), expected.trim());
}

// ── Regression: nested shadowing prevents decomposition ────────────

#[test]
fn inner_scope_shadow_prevents_decomposition() {
    let target_facts = facts_for(r#"export function foo() {}"#);
    let mut facts = ModuleFactsMap::new();
    facts.insert("./mod.js", target_facts);

    // `foo` is a parameter in an inner function — decomposing r.foo → foo
    // would collide with the parameter, producing `foo + foo` instead of
    // `r.foo + foo`.
    let input = r#"
import r from "./mod.js";
function g(foo) { return r.foo + foo; }
"#;
    let output = run_decomp(input, &facts);
    assert!(normalize(&output).contains("import r from"), "should keep default import when inner scope shadows, got: {output}");
}

#[test]
fn catch_param_shadow_prevents_decomposition() {
    let target_facts = facts_for(r#"export function err() {}"#);
    let mut facts = ModuleFactsMap::new();
    facts.insert("./mod.js", target_facts);

    let input = r#"
import r from "./mod.js";
try { r.err(); } catch (err) { console.log(err); }
"#;
    let output = run_decomp(input, &facts);
    assert!(normalize(&output).contains("import r from"), "should keep default import when catch param shadows, got: {output}");
}

#[test]
fn arrow_param_shadow_prevents_decomposition() {
    let target_facts = facts_for(r#"export function x() {}"#);
    let mut facts = ModuleFactsMap::new();
    facts.insert("./mod.js", target_facts);

    let input = r#"
import r from "./mod.js";
const fn = (x) => r.x + x;
"#;
    let output = run_decomp(input, &facts);
    assert!(normalize(&output).contains("import r from"), "should keep default import when arrow param shadows, got: {output}");
}

// ── Regression: mixed imports preserved ────────────────────────────

#[test]
fn mixed_import_preserves_named_specifiers() {
    let target_facts = facts_for(r#"
export function Fragment() {}
export function createElement() {}
export function useState() {}
"#);
    let mut facts = ModuleFactsMap::new();
    facts.insert("./react.js", target_facts);

    let input = r#"
import React, { useState } from "./react.js";
React.createElement("div");
React.Fragment;
useState();
"#;
    let expected = r#"
import { useState, Fragment, createElement } from "./react.js";
createElement("div");
Fragment;
useState();
"#;
    assert_eq_normalized(&run_decomp(input, &facts), expected.trim());
}
