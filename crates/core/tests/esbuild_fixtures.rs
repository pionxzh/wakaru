use std::fs;

use wakaru_core::{unpack, unpack_raw, DecompileOptions};

fn fixture(path: &str) -> String {
    let full = format!("tests/bundles/esbuild-gen/dist/{path}");
    fs::read_to_string(&full).unwrap_or_else(|e| panic!("failed to read {full}: {e}"))
}

fn unpack_fixture(path: &str) -> Vec<(String, String)> {
    let source = fixture(path);
    unpack(
        &source,
        DecompileOptions {
            filename: path.to_string(),
            ..Default::default()
        },
    )
    .unwrap_or_else(|_| panic!("unpack should succeed for {path}"))
}

fn unpack_fixture_raw(path: &str) -> Vec<(String, String)> {
    let source = fixture(path);
    unpack_raw(&source, &DecompileOptions::default())
        .unwrap_or_else(|_| panic!("unpack_raw should succeed for {path}"))
}

fn filenames(pairs: &[(String, String)]) -> Vec<&str> {
    pairs.iter().map(|(n, _)| n.as_str()).collect()
}

fn has_module(pairs: &[(String, String)], needle: &str) -> bool {
    pairs.iter().any(|(name, _)| name.contains(needle))
}

fn module_code<'a>(pairs: &'a [(String, String)], needle: &str) -> &'a str {
    pairs
        .iter()
        .find(|(name, _)| name.contains(needle))
        .map(|(_, code)| code.as_str())
        .unwrap_or_else(|| {
            panic!(
                "expected module containing {needle}, got {:?}",
                filenames(pairs)
            )
        })
}

fn code_containing<'a>(pairs: &'a [(String, String)], needle: &str) -> &'a str {
    pairs
        .iter()
        .find(|(_, code)| code.contains(needle))
        .map(|(_, code)| code.as_str())
        .unwrap_or_else(|| {
            panic!(
                "expected a module containing {needle}, got {:?}",
                filenames(pairs)
            )
        })
}

#[test]
fn es_mixed_extracts_factories_scope_modules_and_decompiles() {
    let raw = unpack_fixture_raw("es-mixed/bundle.js");
    let names = filenames(&raw);

    assert!(
        raw.len() > 1,
        "es-mixed: expected multiple modules, got {}: {names:?}",
        raw.len()
    );
    assert!(
        has_module(&raw, "utils-cjs") || has_module(&raw, "require_utils_cjs"),
        "es-mixed: expected a CJS factory module, got {names:?}"
    );
    assert!(
        has_module(&raw, "math") && has_module(&raw, "greet") && has_module(&raw, "entry"),
        "es-mixed: expected math, greet, and entry modules, got {names:?}"
    );

    let decompiled = unpack_fixture("es-mixed/bundle.js");
    assert!(
        decompiled.len() > 1,
        "es-mixed decompile: expected multiple modules, got {:?}",
        filenames(&decompiled)
    );
}

#[test]
fn iife_factories_remains_single_module_passthrough() {
    let raw = unpack_fixture_raw("iife-factories/bundle.js");
    assert_eq!(
        raw.len(),
        1,
        "iife-factories: expected passthrough, got {:?}",
        filenames(&raw)
    );

    let decompiled = unpack_fixture("iife-factories/bundle.js");
    assert_eq!(
        decompiled.len(),
        1,
        "iife-factories decompile: expected passthrough, got {:?}",
        filenames(&decompiled)
    );
}

#[test]
fn scope_only_bundle_extracts_scope_hoisted_modules() {
    let raw = unpack_fixture_raw("es-scope-only/bundle.js");
    let names = filenames(&raw);

    assert!(
        raw.len() > 1,
        "es-scope-only: expected split modules, got {}: {names:?}",
        raw.len()
    );
    assert!(
        has_module(&raw, "math") && has_module(&raw, "greet") && has_module(&raw, "entry"),
        "es-scope-only: expected math, greet, and entry modules, got {names:?}"
    );
}

#[test]
fn last_scope_module_keeps_module_side_effects() {
    let raw = unpack_fixture_raw("es-scope-side-effects/bundle.js");
    let registry = module_code(&raw, "registry");
    let entry = module_code(&raw, "entry");

    assert!(
        registry.contains("register(") && registry.contains("self"),
        "registry module should keep register(\"self\", ...) side effect:\n{registry}"
    );
    assert!(
        !entry.contains("register("),
        "entry should not absorb registry side effect:\n{entry}"
    );
}

#[test]
fn global_call_side_effect_stays_with_last_module_when_it_references_exports() {
    let raw = unpack_fixture_raw("es-global-side-effect/bundle.js");
    let constants = module_code(&raw, "constants");
    let entry = module_code(&raw, "entry");

    assert!(
        constants.contains("console.log")
            && constants.contains("VALUE")
            && constants.contains("LABEL"),
        "constants module should keep console.log(LABEL, VALUE):\n{constants}"
    );
    assert!(
        !entry.contains("console.log"),
        "entry should not absorb constants side effect:\n{entry}"
    );
}

#[test]
fn entry_expression_after_last_scope_module_stays_in_entry() {
    let raw = unpack_fixture_raw("es-entry-expr/bundle.js");
    let entry = module_code(&raw, "entry");
    let math = module_code(&raw, "math");

    assert!(
        entry.contains("console.log"),
        "entry should contain entry-level console.log:\n{entry}"
    );
    assert!(
        !math.contains("console.log"),
        "math module should not absorb entry-level console.log:\n{math}"
    );
}

#[test]
fn private_helper_before_exports_stays_with_last_module() {
    let raw = unpack_fixture_raw("es-private-helper/bundle.js");
    let helper = module_code(&raw, "helper");
    let entry = module_code(&raw, "entry");

    assert!(
        helper.contains("normalize") && helper.contains("total") && helper.contains("average"),
        "helper module should contain private and exported bindings:\n{helper}"
    );
    assert!(
        entry.contains("main") && !entry.contains("normalize"),
        "entry should keep main and not absorb helper internals:\n{entry}"
    );
}

#[test]
fn single_boundary_bundle_splits_with_esm_export_corroboration() {
    let raw = unpack_fixture_raw("es-single-boundary/bundle.js");
    let names = filenames(&raw);
    let entry = module_code(&raw, "entry");

    assert!(
        raw.len() > 1 && has_module(&raw, "math"),
        "es-single-boundary: expected math plus entry modules, got {names:?}"
    );
    assert!(
        entry.contains("console.log") && entry.contains("entry initialized"),
        "entry should contain entry initialization side effect:\n{entry}"
    );
}

#[test]
fn private_helper_after_export_stays_with_referencing_module() {
    let raw = unpack_fixture_raw("es-helper-after-export/bundle.js");
    let utils = module_code(&raw, "utils");
    let entry = module_code(&raw, "entry");

    assert!(
        utils.contains("compute") && utils.contains("normalize"),
        "utils module should include exported compute and private normalize:\n{utils}"
    );
    assert!(
        entry.contains("main") && !entry.contains("normalize"),
        "entry should keep main and not absorb utils internals:\n{entry}"
    );
}

#[test]
fn bun_minified_scope_only_bundle_extracts_scope_modules() {
    let raw = unpack_fixture_raw("bun-scope-only-min/bundle.js");
    let names = filenames(&raw);
    let entry = module_code(&raw, "entry");

    assert_eq!(
        raw.len(),
        3,
        "bun-scope-only-min: expected two scope modules plus entry, got {names:?}"
    );
    assert!(
        entry.contains("export") && entry.contains("math") && entry.contains("greet"),
        "entry should keep Bun namespace exports:\n{entry}"
    );
    code_containing(&raw, "3.14159");
    code_containing(&raw, "Hello");
}

#[test]
fn bun_minified_side_effect_stays_with_scope_module() {
    let raw = unpack_fixture_raw("bun-scope-side-effects-min/bundle.js");
    let registry = code_containing(&raw, "self");
    let entry = module_code(&raw, "entry");

    assert!(
        registry.contains("loaded") && !entry.contains("loaded"),
        "registry side effect should stay out of entry:\nentry:\n{entry}\nregistry:\n{registry}"
    );
}

#[test]
fn bun_minified_mixed_splits_scope_modules_and_keeps_inlined_cjs_in_entry() {
    let raw = unpack_fixture_raw("bun-mixed-min/bundle.js");
    let names = filenames(&raw);
    let entry = module_code(&raw, "entry");

    assert_eq!(
        raw.len(),
        3,
        "bun-mixed-min: expected math, greet, and entry modules, got {names:?}"
    );
    assert!(
        entry.contains("Math.min") && entry.contains("Object.keys"),
        "Bun inlines CJS helpers, so they should remain in entry:\n{entry}"
    );
    code_containing(&raw, "3.14159");
    code_containing(&raw, "Hello");
}

#[test]
fn bun_minified_single_boundary_splits_with_esm_export_corroboration() {
    let raw = unpack_fixture_raw("bun-single-boundary-min/bundle.js");
    let names = filenames(&raw);
    let entry = module_code(&raw, "entry");

    assert_eq!(
        raw.len(),
        2,
        "bun-single-boundary-min: expected one scope module plus entry, got {names:?}"
    );
    assert!(
        entry.contains("entry initialized") && entry.contains("export"),
        "entry should keep entry expression and namespace export:\n{entry}"
    );
}

#[test]
fn bun_minified_private_helper_after_export_stays_with_module() {
    let raw = unpack_fixture_raw("bun-helper-after-export-min/bundle.js");
    let utils = code_containing(&raw, "Math.abs");
    let entry = module_code(&raw, "entry");

    assert!(
        utils.contains("Math.abs") && !entry.contains("Math.abs"),
        "private helper should stay with Bun utils module:\nentry:\n{entry}\nutils:\n{utils}"
    );
}

#[test]
fn bun_minified_without_namespace_boundaries_remains_single_module() {
    let raw = unpack_fixture_raw("bun-factories-min/bundle.js");

    assert_eq!(
        raw.len(),
        1,
        "Bun output without namespace export boundaries should pass through, got {:?}",
        filenames(&raw)
    );
}
