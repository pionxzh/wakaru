use std::fs;
use std::path::{Path, PathBuf};

mod common;

use common::{assert_eq_normalized, render, render_with_level};
use swc_core::common::{sync::Lrc, SourceMap};
use wakaru_core::{decompile, DecompileOptions, RewriteLevel};

#[test]
fn then_rejection_handler_without_promise_provenance_is_preserved() {
    let input = r#"const result = customThenable.then(null, onRejected);"#;

    assert_eq_normalized(&render(input), input);
}

/// A call callee, a `new` callee, and a `class ... extends` clause must be a
/// left-hand-side expression, so an update or arrow operand there keeps its
/// parentheses through emission.
#[test]
fn minimal_level_keeps_parens_required_by_call_new_and_heritage() {
    let input = r#"(x++)();
new (o.n++)();
new (() => {})();
class C extends (() => {}) {}
"#;
    let expected = r#"(x++)();
new (o.n++)();
new (()=>{})();
class C extends (()=>{}) {
}
"#;

    let output = render_with_level(input, RewriteLevel::Minimal);
    assert_eq!(output, expected);

    let cm: Lrc<SourceMap> = Default::default();
    common::parse_module_with_filename(&output, "output.js", cm);
}

/// A string-literal statement after another statement is not a directive.
/// Removing the statement before it must not turn it into one: `"use strict"`
/// would make the function strict.
#[test]
fn string_statement_does_not_become_directive_when_leading_statement_is_removed() {
    let input = r#"function h() { var x; "use strict"; return this === undefined; }
console.log(h());
"#;
    let expected = r#"function h() {
    ("use strict");
    return this === undefined;
}
console.log(h());
"#;

    assert_eq!(render_with_level(input, RewriteLevel::Minimal), expected);
    assert_eq!(render(input), expected);
}

/// A parenthesized string statement is never a directive, and it ends the
/// directive prologue, so the strings after it are not directives either.
#[test]
fn parenthesized_string_statement_keeps_its_parens() {
    let cases = [
        r#"function h() {
    ("use strict");
    return this === undefined;
}
"#,
        r#"function h() {
    ("x");
    "use strict";
    return this === undefined;
}
"#,
        r#"function h() {
    "use asm";
    ("use strict");
    return this === undefined;
}
"#,
        r#"const h = ()=>{
    ("use strict");
    return this;
};
"#,
    ];

    for input in cases {
        assert_eq!(render_with_level(input, RewriteLevel::Minimal), input);
        assert_eq!(render(input), input);
    }
}

/// A string that was an operand of a sequence is not a directive either.
/// Splitting the sequence must not turn it into one.
#[test]
fn string_statement_does_not_become_directive_when_sequence_is_split() {
    let input = r#"function h() { "use strict", g(); return this === undefined; }
"#;
    let expected = r#"function h() {
    ("use strict");
    g();
    return this === undefined;
}
"#;

    assert_eq!(render_with_level(input, RewriteLevel::Minimal), expected);
    assert_eq!(render(input), expected);
}

/// Real directives stay bare, including one after an earlier directive.
#[test]
fn directive_prologue_is_preserved() {
    let input = r#"function h() {
    "use asm";
    "use strict";
    return this === undefined;
}
"#;

    assert_eq!(render_with_level(input, RewriteLevel::Minimal), input);
    assert_eq!(render(input), input);
}

#[test]
fn minimal_level_keeps_parens_for_prefix_update_async_arrow_and_optional_call() {
    let cases = [
        "(++x)();",
        "(x--)();",
        "new (++x)();",
        "new (async () => {})();",
        "class C extends (async () => {}) {}",
        "(x++)?.();",
    ];

    for input in cases {
        let output = render_with_level(input, RewriteLevel::Minimal);
        assert_eq_normalized(&output, input);

        let cm: Lrc<SourceMap> = Default::default();
        common::parse_module_with_filename(&output, "output.js", cm);
    }
}

/// A template tag takes a member expression, and a call or optional call
/// callee must not absorb a `new` that has no argument list.
#[test]
fn minimal_level_keeps_parens_required_by_tag_and_new_callee() {
    let input = r#"(x++)`t`;
(class {})`t`;
(new a)();
(new b())?.();
async function f() {
    (await c)`t`;
}
"#;
    let expected = r#"(x++)`t`;
(class {
})`t`;
(new a)();
(new b())?.();
async function f() {
    (await c)`t`;
}
"#;

    let output = render_with_level(input, RewriteLevel::Minimal);
    assert_eq!(output, expected);

    let cm: Lrc<SourceMap> = Default::default();
    common::parse_module_with_filename(&output, "output.js", cm);
}

#[test]
fn minimal_level_adds_no_parens_to_valid_call_new_and_heritage() {
    let input = r#"foo();
new Foo();
class D extends Foo {}
(x => x)();
o.m++;
"#;
    let expected = r#"foo();
new Foo();
class D extends Foo {
}
((x)=>x)();
o.m++;
"#;

    let output = render_with_level(input, RewriteLevel::Minimal);

    assert_eq!(output, expected);
}

#[test]
fn decompile_handles_existing_bundled_fixtures() {
    for fixture in bundled_fixture_paths() {
        let input = fs::read_to_string(&fixture).expect("fixture should be readable");
        let filename = fixture
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("index.js")
            .to_string();

        let output = decompile(
            &input,
            DecompileOptions {
                filename,
                ..Default::default()
            },
        )
        .expect("decompile should succeed")
        .code;
        assert!(
            !output.trim().is_empty(),
            "output should not be empty for {}",
            fixture.display()
        );
    }
}

#[test]
fn decompile_output_is_stable_for_noop_pipeline() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("testcases")
        .join("browserify")
        .join("dist")
        .join("index.js");

    let input = fs::read_to_string(&fixture).expect("fixture should be readable");
    let once = decompile(
        &input,
        DecompileOptions {
            filename: "index.js".to_string(),
            ..Default::default()
        },
    )
    .expect("first decompile should succeed")
    .code;
    let twice = decompile(
        &once,
        DecompileOptions {
            filename: "index.js".to_string(),
            ..Default::default()
        },
    )
    .expect("second decompile should succeed")
    .code;

    assert_eq!(common::normalize(&once), common::normalize(&twice));
}

#[test]
fn decompile_parses_jsx_in_js_files() {
    let output = decompile(
        "const view = <div className=\"ok\" />;",
        DecompileOptions {
            filename: "view.js".to_string(),
            ..Default::default()
        },
    )
    .expect("jsx in .js should parse")
    .code;

    insta::assert_snapshot!(output);
}

fn bundled_fixture_paths() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("testcases");
    let mut paths = Vec::new();

    for entry in fs::read_dir(root).expect("testcases should exist") {
        let entry = entry.expect("testcase entry should be readable");
        let path = entry.path().join("dist").join("index.js");
        if path.exists() {
            paths.push(path);
        }
    }

    paths.sort();
    paths
}
