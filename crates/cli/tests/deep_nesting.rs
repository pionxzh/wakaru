//! The CLI binary runs its work on threads whose stacks hold deeply nested
//! expressions. These spawn the binary itself: the thread setup lives in
//! `main`, which the in-crate tests never reach.

#![allow(clippy::disallowed_methods, reason = "tests build their own fixtures")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Deep enough to overflow the platform default stacks (1 MiB on the
/// Windows main thread, 2 MiB on a spawned thread) in any build profile.
const DEPTH: usize = 2000;

fn scratch_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("wakaru-deep-nesting-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir should be created");
    dir
}

fn run_wakaru(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wakaru"))
        .args(args)
        .output()
        .expect("wakaru should start")
}

fn assert_succeeded(output: &Output) {
    assert!(
        output.status.success(),
        "wakaru failed with {}:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `a.b().c("k0").c("k1")…`: each call nests the chain two levels deeper.
fn method_chain(len: usize) -> String {
    let mut chain = String::from("a.b()");
    for i in 0..len {
        chain.push_str(&format!(".c(\"k{i}\")"));
    }
    chain
}

/// `"s0" + v + "s1" + v + …`: a left-nested `+` chain.
fn concatenation(len: usize) -> String {
    let mut concat = String::from("\"s0\"");
    for i in 1..len {
        concat.push_str(&format!(" + v + \"s{i}\""));
    }
    concat
}

#[test]
fn decompiles_a_long_method_chain() {
    let dir = scratch_dir("chain");
    let input = dir.join("input.js");
    let output = dir.join("output.js");
    std::fs::write(
        &input,
        format!("export const config = {};\n", method_chain(DEPTH)),
    )
    .unwrap();

    let result = run_wakaru(&[&input, Path::new("-o"), &output]);

    assert_succeeded(&result);
    let code = std::fs::read_to_string(&output).unwrap();
    assert!(
        code.contains(&format!("\"k{}\"", DEPTH - 1)),
        "the last call of the chain is missing from the output"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unpacks_modules_with_long_expressions() {
    let dir = scratch_dir("unpack");
    let input = dir.join("bundle.js");
    let out = dir.join("out");
    // A minified webpack 4 bootstrap whose modules hold the deep expressions,
    // so the module pipeline runs them on the worker pool.
    let bundle = format!(
        "!function(e){{var n={{}};function r(t){{if(n[t])return n[t].exports;var o=n[t]={{i:t,l:!1,exports:{{}}}};return e[t].call(o.exports,o,o.exports,r),o.l=!0,o.exports}}r.m=e,r.c=n,r.d=function(e,n,t){{r.o(e,n)||Object.defineProperty(e,n,{{enumerable:!0,get:t}})}},r.o=function(e,n){{return Object.prototype.hasOwnProperty.call(e,n)}},r.p=\"\",r(r.s=0)}}([function(e,n,r){{var v=r(1);globalThis.css={}}},function(e,n){{n.config={}}}]);",
        concatenation(DEPTH),
        method_chain(DEPTH),
    );
    std::fs::write(&input, bundle).unwrap();

    let result = run_wakaru(&[&input, Path::new("--unpack"), Path::new("-o"), &out]);

    assert_succeeded(&result);
    let modules: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap_or_default())
        .collect();
    assert!(
        modules.len() >= 2,
        "expected the two modules to be split out, got {}",
        modules.len()
    );
    let all = modules.concat();
    // The concatenation may come back as a template literal, so look for the
    // last string's text rather than its quoted form.
    assert!(
        all.contains(&format!("s{}", DEPTH - 1)),
        "the last term of the concatenation is missing from the output"
    );
    assert!(
        all.contains(&format!("\"k{}\"", DEPTH - 1)),
        "the last call of the chain is missing from the output"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
