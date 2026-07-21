//! With `output.iife: false` or `experiments.outputModule`, webpack 5 emits
//! the bootstrap statements at the top level of the file instead of inside an
//! IIFE. Detection must recognize this unwrapped form.

use wakaru_core::{unpack, DecompileOptions};

fn expect_unpack(source: &str, filename: &str) -> Vec<(String, String)> {
    let output = unpack(
        source,
        DecompileOptions {
            filename: filename.to_string(),
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

const NO_IIFE_BODY: &str = r#"
var __webpack_modules__ = ([
    ,
    ((__unused_webpack_module, exports) => {
        exports.greet = function (name) { return "hi " + name; };
    }),
    ((__unused_webpack_module, exports, __webpack_require__) => {
        var dep = __webpack_require__(1);
        exports.shout = function () { return dep.greet("x"); };
    })
]);
const __webpack_module_cache__ = {};
function __webpack_require__(moduleId) {
    const cachedModule = __webpack_module_cache__[moduleId];
    if (cachedModule !== undefined) {
        return cachedModule.exports;
    }
    const module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
}
__webpack_require__.m = __webpack_modules__;
"#;

#[test]
fn webpack5_no_iife_splits_modules() {
    let pairs = expect_unpack(NO_IIFE_BODY, "bundle.js");
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();

    assert!(
        filenames.contains(&"module-1.js") && filenames.contains(&"module-2.js"),
        "unwrapped bootstrap should split into modules, got {filenames:?}"
    );

    let mod_2 = pairs
        .iter()
        .find(|(name, _)| name == "module-2.js")
        .expect("module-2.js should exist");
    assert!(
        mod_2.1.contains("./module-1.js"),
        "module-2 should reference ./module-1.js, got:\n{}",
        mod_2.1
    );
}

#[test]
fn webpack5_no_iife_with_inline_entry() {
    let source = format!(
        "{NO_IIFE_BODY}\nvar __webpack_exports__ = {{}};\nvar lib = __webpack_require__(2);\nconsole.log(lib.shout());\n"
    );
    let pairs = expect_unpack(&source, "bundle.js");
    let entry = pairs
        .iter()
        .find(|(name, _)| name == "entry.js")
        .unwrap_or_else(|| {
            panic!(
                "entry.js should exist, got {:?}",
                pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
            )
        });
    assert!(
        entry.1.contains("./module-2.js"),
        "no-IIFE inline entry should be recovered, got:\n{}",
        entry.1
    );
}

#[test]
fn webpack5_output_module_with_top_level_exports_is_not_extracted() {
    // experiments.outputModule drops the IIFE but emits top-level ESM
    // export/import declarations carrying the library's public surface.
    // Faithfully recovering those needs harmony-export reconstruction the
    // pipeline does not do, so such a bundle must be left untouched rather than
    // split into an entry that silently loses its exports.
    let source = format!(
        "{NO_IIFE_BODY}\nlet __webpack_exports__ = {{}};\nvar run = __webpack_require__(2);\nexport {{ run }};\n"
    );
    let pairs = expect_unpack(&source, "bundle.mjs");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "output.module bundle with top-level exports must not be split, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn plain_top_level_script_is_not_webpack5() {
    // A top-level script with a var + function must not be mistaken for an
    // unwrapped webpack bundle.
    let source = r#"
var handlers = [
    function onClick() { return 1; },
    function onHover() { return 2; }
];
function dispatch(i) { return handlers[i](); }
console.log(dispatch(0));
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "plain script must not unpack as webpack5, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}
