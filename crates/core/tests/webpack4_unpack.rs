mod common;

use std::fs;

use wakaru_core::driver::test_support::{unpack, unpack_raw};
use wakaru_core::{validate_output_modules, DecompileOptions};

#[test]
fn webpack4_unpack_extracts_modules() {
    let source_path = "../../testcases/webpack4/dist/index.js";
    let source = fs::read_to_string(source_path)
        .expect("failed to read webpack4 testcase — make sure the testcases are present");

    let output = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let pairs = output.modules;

    // Must extract at least 50 modules
    assert!(
        pairs.len() >= 50,
        "expected at least 50 modules, got {}",
        pairs.len()
    );

    // Each module must have non-empty code
    for (filename, code) in &pairs {
        assert!(!code.trim().is_empty(), "module {filename} has empty code");
    }

    // The entry module must exist
    let has_entry = pairs.iter().any(|(name, _)| name == "entry.js");
    assert!(
        has_entry,
        "no entry.js module found; filenames: {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn webpack4_raw_unpack_extracts_modules_without_pipeline() {
    let source_path = "../../testcases/webpack4/dist/index.js";
    let source = fs::read_to_string(source_path)
        .expect("failed to read webpack4 testcase — make sure the testcases are present");

    let output =
        unpack_raw(&source, &DecompileOptions::default()).expect("raw unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let pairs = output.modules;

    assert!(
        pairs.len() >= 50,
        "expected at least 50 modules, got {}",
        pairs.len()
    );
    assert!(
        pairs.iter().any(|(name, _)| name == "entry.js"),
        "no entry.js module found; filenames: {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    assert!(
        pairs.iter().all(|(_, code)| !code.trim().is_empty()),
        "raw unpack should not produce empty modules"
    );

    let decompiled_output = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
            ..Default::default()
        },
    )
    .expect("decompiled unpack should succeed");
    assert!(
        !decompiled_output.has_errors(),
        "unexpected warnings: {:?}",
        decompiled_output.warnings
    );
    let decompiled_pairs = decompiled_output.modules;

    assert!(
        pairs
            .iter()
            .any(|(filename, raw_code)| decompiled_pairs.iter().any(
                |(decompiled_filename, decompiled_code)| filename == decompiled_filename
                    && raw_code != decompiled_code
            )),
        "raw unpack should preserve at least one pre-pipeline module difference"
    );
}

#[test]
fn webpack4_empty_factory_recovers_runtime_default_object_only_in_normal_output() {
    let source = r#"
(function(modules) {
    var installedModules = {};
    function __webpack_require__(moduleId) {
        if (installedModules[moduleId]) {
            return installedModules[moduleId].exports;
        }
        var module = installedModules[moduleId] = { exports: {} };
        modules[moduleId].call(module.exports, module, module.exports, __webpack_require__);
        return module.exports;
    }
    return __webpack_require__(0);
})([
    function(module, exports, __webpack_require__) {
        var dependency = __webpack_require__(1);
        module.exports = dependency.missing;
    },
    function(module, exports, __webpack_require__) {},
    function(module, exports, __webpack_require__) {
        observeSideEffect();
    }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 empty-factory bundle should unpack");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
    let provider = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-1.js")
        .map(|(_, code)| code.as_str())
        .expect("empty factory should still be emitted");
    assert_eq!(provider.trim(), "export default {};");
    let side_effect_only = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-2.js")
        .map(|(_, code)| code.as_str())
        .expect("non-empty side-effect factory should still be emitted");
    assert!(
        side_effect_only.contains("observeSideEffect()")
            && !side_effect_only.contains("export default"),
        "the runtime fact must not be generalized to non-empty factories:\n{side_effect_only}"
    );

    let raw = unpack_raw(
        source,
        &DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("raw webpack4 empty-factory bundle should unpack");
    let raw_provider = raw
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-1.js")
        .map(|(_, code)| code.as_str())
        .expect("raw output should retain the empty factory module");
    assert!(
        raw_provider.trim().is_empty(),
        "raw output must remain detector passthrough, got:\n{raw_provider}"
    );
}

/// Snapshot test: every extracted module's decompiled output is pinned.
#[test]
fn webpack4_unpack_snapshots() {
    let source_path = "../../testcases/webpack4/dist/index.js";
    let source = fs::read_to_string(source_path)
        .expect("failed to read webpack4 testcase — make sure the testcases are present");

    let output = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let mut pairs = output.modules;

    // Sort for stable snapshot order
    pairs.sort_by(|(a, _), (b, _)| a.cmp(b));

    // Use the filename (without extension) as the snapshot name
    common::assert_each_snapshot(
        pairs
            .iter()
            .map(|(filename, code)| (filename.trim_end_matches(".js"), code)),
        |name, code| insta::assert_snapshot!(name, code),
    );
}

#[test]
fn webpack4_unpacks_array_concat_offset_modules() {
    // webpack renders `Array(minId).concat([...])` when dense numeric module
    // ids start above zero (Template.getModulesArrayBounds); element indices
    // are offset by minId.
    let source = r#"
!function(modules) {
    var installedModules = {};
    function __webpack_require__(moduleId) {
        if (installedModules[moduleId]) {
            return installedModules[moduleId].exports;
        }
        var module = installedModules[moduleId] = { i: moduleId, l: false, exports: {} };
        modules[moduleId].call(module.exports, module, module.exports, __webpack_require__);
        module.l = true;
        return module.exports;
    }
    return __webpack_require__(__webpack_require__.s = 30);
}(Array(30).concat([
    function(module, exports, __webpack_require__) {
        var dep = __webpack_require__(31);
        module.exports = dep.value + 1;
    },
    function(module, exports) {
        exports.value = 41;
    }
]));
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let pairs = output.modules;
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();

    // Module id 30 is the entry (require(30) in the bootstrap); id 31 follows.
    assert!(
        filenames.contains(&"entry.js") && filenames.contains(&"module-31.js"),
        "concat-form ids should be offset by Array(n), got {filenames:?}"
    );

    let entry = pairs
        .iter()
        .find(|(name, _)| name == "entry.js")
        .expect("entry.js should exist");
    assert!(
        entry.1.contains("./module-31.js"),
        "entry should reference ./module-31.js, got:\n{}",
        entry.1
    );
}

#[test]
fn webpack4_unpacks_parenthesized_array_factories() {
    // Unminified webpack 4 wraps each factory in parens: `/***/ (function(...) {...})`.
    let source = r#"
(function(modules) {
    var installedModules = {};
    function __webpack_require__(moduleId) {
        if (installedModules[moduleId]) {
            return installedModules[moduleId].exports;
        }
        var module = installedModules[moduleId] = { i: moduleId, l: false, exports: {} };
        modules[moduleId].call(module.exports, module, module.exports, __webpack_require__);
        module.l = true;
        return module.exports;
    }
    return __webpack_require__(__webpack_require__.s = 0);
})([
    (function(module, exports, __webpack_require__) {
        var dep = __webpack_require__(1);
        module.exports = dep.value * 2;
    }),
    (function(module, exports) {
        exports.value = 21;
    })
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let pairs = output.modules;
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();

    assert!(
        filenames.contains(&"entry.js") && filenames.contains(&"module-1.js"),
        "parenthesized factories should be recognized, got {filenames:?}"
    );
}

#[test]
fn assert_each_snapshot_checks_every_pair_and_names_each_failure() {
    let checked = std::cell::RefCell::new(Vec::new());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        common::assert_each_snapshot(
            [("first", "ok"), ("second", "bad"), ("third", "bad")],
            |name, code| {
                checked.borrow_mut().push(name.to_string());
                assert_eq!(code, "ok");
            },
        );
    }));
    let message = result
        .expect_err("a failed pair should fail the helper")
        .downcast::<String>()
        .expect("panic message should be a String");
    assert_eq!(*checked.borrow(), ["first", "second", "third"]);
    assert!(message.contains("2 snapshot(s) failed: second, third"));
}

// ---------------------------------------------------------------------------
// Code around the bootstrap
// ---------------------------------------------------------------------------

/// A minified webpack 4 bootstrap whose startup requires module 0, which
/// assigns module 1's exports to a global.
const WEBPACK4_BOOTSTRAP: &str = r#"function(e){var n={};function r(t){if(n[t])return n[t].exports;var o=n[t]={i:t,l:!1,exports:{}};return e[t].call(o.exports,o,o.exports,r),o.l=!0,o.exports}r.m=e,r.c=n,r.d=function(e,n,t){r.o(e,n)||Object.defineProperty(e,n,{enumerable:!0,get:t})},r.o=function(e,n){return Object.prototype.hasOwnProperty.call(e,n)},r.p="",r(r.s=0)}([function(e,n,r){globalThis.lib=r(1)},function(e,n){n.twice=function(e){return 2*e}}])"#;

const BANNER: &str =
    r#"!function(){try{globalThis.banner={v:1}}catch(e){console.error("bad banner")}}()"#;

fn expect_pairs(source: &str) -> Vec<(String, String)> {
    let output = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    output.modules
}

fn named<'a>(pairs: &'a [(String, String)], name: &str) -> &'a str {
    &pairs
        .iter()
        .find(|(file, _)| file == name)
        .unwrap_or_else(|| {
            panic!(
                "{name} should exist, got {:?}",
                pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
            )
        })
        .1
}

fn assert_banner_runs_around_the_entry(pairs: &[(String, String)]) {
    let entry = named(pairs, "entry.js");
    assert!(
        entry.contains("globalThis.banner = {"),
        "the banner must stay in entry.js, got:\n{entry}"
    );
    assert!(
        entry.contains("./entry-0.js"),
        "entry.js must load the entry module, got:\n{entry}"
    );
    assert!(
        named(pairs, "entry-0.js").contains("globalThis.lib"),
        "the entry module keeps its own code"
    );
    assert!(pairs.iter().any(|(name, _)| name == "module-1.js"));
    assert!(
        validate_output_modules(pairs).is_empty(),
        "output must validate: {:?}",
        validate_output_modules(pairs)
    );
}

#[test]
fn banner_merged_into_a_sequence_with_the_bootstrap_stays_in_entry() {
    // shape: producer webpack@4.47.0 mode=production (Terser), BannerPlugin
    // raw=true (webpack 4's BannerPlugin runs before minification, so Terser
    // merges the banner and the bootstrap into one statement); globals
    // renamed to `globalThis.banner` / `globalThis.lib`.
    let source = format!("{BANNER},{WEBPACK4_BOOTSTRAP};");

    assert_banner_runs_around_the_entry(&expect_pairs(&source));
}

#[test]
fn code_before_the_bootstrap_statement_stays_in_entry() {
    // A script concatenated in front of the bundle. shape: hypothetical.
    let source = format!("{BANNER};\n!{WEBPACK4_BOOTSTRAP};");

    assert_banner_runs_around_the_entry(&expect_pairs(&source));
}

#[test]
fn code_after_the_bootstrap_statement_follows_the_entry() {
    // shape: hypothetical (a script concatenated after the bundle).
    let source = format!("!{WEBPACK4_BOOTSTRAP};\nglobalThis.after = 2;");

    let pairs = expect_pairs(&source);
    let entry = named(&pairs, "entry.js");
    let load = entry
        .find("./entry-0.js")
        .unwrap_or_else(|| panic!("entry.js must load the entry module, got:\n{entry}"));
    let after = entry
        .find("globalThis.after = 2")
        .unwrap_or_else(|| panic!("the code after must stay, got:\n{entry}"));
    assert!(load < after, "source order must be kept, got:\n{entry}");
}

#[test]
fn code_around_a_webpack4_bootstrap_declaring_require_stays_unsplit() {
    // entry.js calls `require`; a top-level `require` declared around the
    // bootstrap would capture it. shape: hypothetical.
    let source = format!("var require = function () {{}};\n!{WEBPACK4_BOOTSTRAP};");

    let pairs = expect_pairs(&source);
    assert_eq!(
        pairs.len(),
        1,
        "the input must stay one file, got {pairs:?}"
    );
}

// ---------------------------------------------------------------------------
// Code around the bootstrap that a factory uses
// ---------------------------------------------------------------------------

/// A string-array decoder declared at the top level before the bundle.
/// shape: hypothetical, reduced from producer javascript-obfuscator@5.9.0
/// --string-array true --string-array-threshold 1 --compact true over a
/// producer webpack@1.15.0 -p bundle, which declares the array function and
/// its decoder at the top level and calls the decoder in every factory.
const DECODER: &str =
    r#"var strings=["lib","call","label"];var decode=function(i){return strings[i]};"#;

fn names(pairs: &[(String, String)]) -> Vec<&str> {
    pairs.iter().map(|(name, _)| name.as_str()).collect()
}

#[test]
fn factory_reading_a_declaration_around_the_bootstrap_stays_unsplit() {
    // The factory calls `decode` as a global. Split, `decode` would be
    // module-scoped in entry.js and undefined in the factory's file.
    let bootstrap = WEBPACK4_BOOTSTRAP.replace("globalThis.lib=", "globalThis[decode(0)]=");
    let source = format!("{DECODER}\n!{bootstrap};");

    let pairs = expect_pairs(&source);
    assert_eq!(
        pairs.len(),
        1,
        "the input must stay one file, got {:?}",
        names(&pairs)
    );
}

#[test]
fn declaration_around_the_bootstrap_used_only_outside_the_factories_splits() {
    // Only the bootstrap's require function and the code after the bundle
    // call the decoder; no factory does.
    let bootstrap = WEBPACK4_BOOTSTRAP.replace("e[t].call(", "e[t][decode(1)](");
    let source = format!("{DECODER}\n!{bootstrap};\nglobalThis.label = decode(2);");

    let pairs = expect_pairs(&source);
    assert_eq!(names(&pairs), ["entry-0.js", "module-1.js", "entry.js"]);
    // entry.js becomes ESM, so the require of the entry module is hoisted
    // as an import (see `import_hoisting_eagerness`).
    let entry = named(&pairs, "entry.js");
    assert!(
        entry.contains(r#"import "./entry-0.js";"#),
        "entry.js must load the entry module, got:\n{entry}"
    );
    let declaration = entry
        .find("const decode =")
        .unwrap_or_else(|| panic!("the decoder must stay in entry.js, got:\n{entry}"));
    let read = entry
        .find("globalThis.label = decode(2)")
        .unwrap_or_else(|| panic!("the code after must stay, got:\n{entry}"));
    assert!(
        declaration < read,
        "source order must be kept, got:\n{entry}"
    );
    assert!(
        validate_output_modules(&pairs).is_empty(),
        "output must validate: {:?}",
        validate_output_modules(&pairs)
    );
}

#[test]
fn factory_binding_named_like_a_declaration_around_the_bootstrap_splits() {
    // The factory declares its own `decode`; it never reads the global one.
    let bootstrap = WEBPACK4_BOOTSTRAP.replace(
        "globalThis.lib=r(1)",
        r#"var decode=function(){return"lib"};globalThis[decode()]=r(1)"#,
    );
    let source = format!("{DECODER}\n!{bootstrap};");

    let pairs = expect_pairs(&source);
    assert_eq!(names(&pairs), ["entry-0.js", "module-1.js", "entry.js"]);
}

// ---------------------------------------------------------------------------
// webpack 1 startup
// ---------------------------------------------------------------------------

/// webpack 1's minified bootstrap: the startup is the last element of the
/// bootstrap's `return`, a plain call of the require function with the entry
/// id. shape: producer webpack@1.15.0 -p; global renamed to `globalThis.lib`.
const WEBPACK1_BOOTSTRAP: &str = r#"function(r){function t(o){if(n[o])return n[o].exports;var e=n[o]={exports:{},id:o,loaded:!1};return r[o].call(e.exports,e,e.exports,t),e.loaded=!0,e.exports}var n={};return t.m=r,t.c=n,t.p="",t(0)}([function(r,t,n){var o=n(1);globalThis.lib=o(21)},function(r,t){r.exports=function(r){return 2*r}}])"#;

#[test]
fn webpack1_startup_call_marks_the_entry_module() {
    let output = unpack(
        &format!("!{WEBPACK1_BOOTSTRAP};"),
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert_eq!(names(&output.modules), ["entry.js", "module-1.js"]);
    assert!(
        named(&output.modules, "entry.js").contains("globalThis.lib"),
        "module 0 is the entry, got:\n{}",
        named(&output.modules, "entry.js")
    );
    assert!(
        output
            .provenance
            .iter()
            .all(|module| module.is_entry == (module.filename == "entry.js")),
        "only the started module is the entry: {:?}",
        output.provenance
    );
}

#[test]
fn unminified_webpack1_startup_statement_marks_the_entry_module() {
    // shape: producer webpack@1.15.0 (no -p), comments trimmed.
    let source = r#"
(function(modules) {
	var installedModules = {};
	function __webpack_require__(moduleId) {
		if(installedModules[moduleId])
			return installedModules[moduleId].exports;
		var module = installedModules[moduleId] = {
			exports: {},
			id: moduleId,
			loaded: false
		};
		modules[moduleId].call(module.exports, module, module.exports, __webpack_require__);
		module.loaded = true;
		return module.exports;
	}
	__webpack_require__.m = modules;
	__webpack_require__.c = installedModules;
	__webpack_require__.p = "";
	return __webpack_require__(0);
})([
function(module, exports, __webpack_require__) {
	var twice = __webpack_require__(1);
	globalThis.lib = twice(21);
},
function(module, exports) {
	module.exports = function (x) { return 2 * x; };
}
]);
"#;

    assert_eq!(names(&expect_pairs(source)), ["entry.js", "module-1.js"]);
}

#[test]
fn concatenated_webpack1_bundles_keep_the_later_ones_in_entry() {
    // Two bundles in one file: the first is split, the second runs after it
    // from entry.js. shape: hypothetical (producer webpack@1.15.0 -p output,
    // concatenated).
    let second = WEBPACK1_BOOTSTRAP.replace("globalThis.lib", "globalThis.other");
    let source = format!("!{WEBPACK1_BOOTSTRAP};!{second};");

    let pairs = expect_pairs(&source);
    assert_eq!(names(&pairs), ["entry-0.js", "module-1.js", "entry.js"]);
    let entry = named(&pairs, "entry.js");
    let load = entry
        .find("./entry-0.js")
        .unwrap_or_else(|| panic!("entry.js must load the entry module, got:\n{entry}"));
    let later = entry
        .find("globalThis.other")
        .unwrap_or_else(|| panic!("the second bundle must stay, got:\n{entry}"));
    assert!(load < later, "source order must be kept, got:\n{entry}");
}

#[test]
fn webpack1_startup_must_be_the_last_returned_call_of_the_require_function() {
    // Each variant has code around the bootstrap, so a startup the detector
    // does not accept keeps the input one file. shape: hypothetical.
    let cases = [
        // Not last in the returned sequence.
        WEBPACK1_BOOTSTRAP.replace(r#"t.p="",t(0)"#, r#"t(0),t.p="""#),
        // Nested in a function that may never run.
        WEBPACK1_BOOTSTRAP.replace(
            r#"return t.m=r,t.c=n,t.p="",t(0)"#,
            r#"return t.m=r,t.c=n,t.p="",function(){return t(0)}"#,
        ),
        // A declared function that does not run a module.
        WEBPACK1_BOOTSTRAP.replace(
            r#"var n={};return t.m=r,t.c=n,t.p="",t(0)"#,
            r#"var n={};function u(o){return o}return t.m=r,t.c=n,t.p="",u(0)"#,
        ),
        // More than one argument.
        WEBPACK1_BOOTSTRAP.replace(r#"t.p="",t(0)"#, r#"t.p="",t(0,1)"#),
    ];
    for bootstrap in cases {
        assert!(
            bootstrap != WEBPACK1_BOOTSTRAP,
            "the replacement must apply"
        );
        let pairs = expect_pairs(&format!("{BANNER};\n!{bootstrap};"));
        assert_eq!(
            pairs.len(),
            1,
            "no entry is recognized, so the input stays one file, got {:?}",
            names(&pairs)
        );
    }
}

#[test]
fn obfuscated_webpack1_dispatch_still_marks_the_entry_module() {
    // The obfuscator turns the require function's `.call` into a decoded
    // computed member; only the runtime and the code after the bundle call
    // the decoder.
    let bootstrap = WEBPACK1_BOOTSTRAP.replace("r[o].call(", "r[o][decode(1)](");
    let source = format!("{DECODER}\n!{bootstrap};\nglobalThis.label = decode(2);");

    let pairs = expect_pairs(&source);
    assert_eq!(names(&pairs), ["entry-0.js", "module-1.js", "entry.js"]);
}

#[test]
fn obfuscated_webpack1_bundle_whose_factories_call_the_decoder_stays_unsplit() {
    // With its startup recognized, the bundle would split; the factories'
    // decoder calls keep it one file.
    let bootstrap = WEBPACK1_BOOTSTRAP
        .replace("r[o].call(", "r[o][decode(1)](")
        .replace("globalThis.lib=", "globalThis[decode(0)]=");
    let source = format!("{DECODER}\n!{bootstrap};");

    let pairs = expect_pairs(&source);
    assert_eq!(
        pairs.len(),
        1,
        "the input must stay one file, got {:?}",
        names(&pairs)
    );
}
