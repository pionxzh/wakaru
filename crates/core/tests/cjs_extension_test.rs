//! An explicit `.cjs`/`.cts` input is decompiled as CommonJS (#225).
//!
//! The extension names the source goal: Node loads a `.cjs` file as CommonJS
//! whatever it contains, so recovering `import`/`export` into one produces a
//! file the runtime it is meant for cannot load. The output validator already
//! reads the extension that way (`filename_source_goal`); the rewrite pipeline
//! must not undo it for the file being decompiled -- and only for the file being
//! decompiled: unpack's phases also stop at `UnEsm`, where a `.cjs` name is a
//! recovered resource inside the bundle and the graph is ESM either way.

use wakaru_core::{decompile, DecompileOptions};

mod common;

const CJS_INPUT: &str = r#""use strict";
const value = require("./value.cjs");
module.exports = { value };
"#;

fn run(filename: &str) -> String {
    decompile(
        CJS_INPUT,
        DecompileOptions {
            filename: filename.to_string(),
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code
}

#[test]
fn cjs_input_keeps_its_require_and_module_exports() {
    let output = run("fixture.cjs");
    assert!(
        output.contains("require("),
        ".cjs must keep its require call: {output}"
    );
    assert!(
        output.contains("module.exports"),
        ".cjs must keep its module.exports: {output}"
    );
    assert!(
        !output.contains("import ") && !output.contains("export "),
        ".cjs must not gain ESM syntax: {output}"
    );
}

/// `.cts` names TypeScript CommonJS the same way.
#[test]
fn cts_input_keeps_its_require_and_module_exports() {
    let output = run("fixture.cts");
    assert!(
        output.contains("require(") && output.contains("module.exports"),
        ".cts must keep its CommonJS shape: {output}"
    );
}

/// The gate is the extension, not the content: the same source under a `.js`
/// name still recovers ESM, which is what the pipeline is for.
#[test]
fn js_input_still_recovers_esm() {
    let output = run("fixture.js");
    assert!(
        output.contains("import value from"),
        ".js must still recover the import: {output}"
    );
    assert!(
        output.contains("export default"),
        ".js must still recover the export: {output}"
    );
}

/// Unpack's first phases run the pipeline up to and including `UnEsm` with no
/// module facts, and there a `.cjs` is a recovered resource name inside the
/// bundle, not the goal of the file being decompiled. Standing down there would
/// leave that module CommonJS, calling a webpack runtime the output does not
/// define (`require.r(...)`, `require.d(...)`) beside modules that import it.
/// Pinned through the same options that phase builds.
#[test]
fn through_un_esm_phase_still_recovers_esm_for_a_cjs_name() {
    let output = common::render_pipeline_until_with_filename(CJS_INPUT, "UnEsm", "version.cjs");
    assert!(
        output.contains("import "),
        "a recovered .cjs name must still recover ESM in that phase: {output}"
    );
    assert!(
        !output.contains("require("),
        "no CommonJS require may survive that phase: {output}"
    );
}
