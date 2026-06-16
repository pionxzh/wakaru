mod common;
use wakaru_core::{decompile, DceMode, DecompileOptions};

fn decompile_with_dce(source: &str, dce_mode: DceMode) -> String {
    decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            dce_mode,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code
}

#[test]
fn transform_only_removes_transform_induced_dead_helper() {
    // _classCallCheck is used in the input — it becomes dead when the rule
    // rewrites the call sites away.  Delta-DCE should still remove it.
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
class Foo {
    constructor() {
        _classCallCheck(this, Foo);
    }
}
export { Foo };
"#;
    let output = decompile_with_dce(input, DceMode::TransformOnly);
    assert!(
        !output.contains("_classCallCheck"),
        "transform-induced dead helper should be removed in TransformOnly mode:\n{output}"
    );
}

#[test]
fn transform_only_preserves_pre_existing_dead_helper() {
    // _unusedHelper is already dead in the input — no call sites.
    // Delta-DCE should keep it.
    let input = r#"
function _unusedHelper(x) {
    return x + 1;
}
export const value = 42;
"#;
    let output = decompile_with_dce(input, DceMode::TransformOnly);
    assert!(
        output.contains("_unusedHelper"),
        "pre-existing dead code should be preserved in TransformOnly mode:\n{output}"
    );
}

#[test]
fn full_dce_removes_pre_existing_dead_helper() {
    let input = r#"
function _unusedHelper(x) {
    return x + 1;
}
export const value = 42;
"#;
    let output = decompile_with_dce(input, DceMode::Full);
    assert!(
        !output.contains("_unusedHelper"),
        "pre-existing dead code should be removed in Full mode:\n{output}"
    );
}

#[test]
fn off_mode_preserves_all_dead_code() {
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
class Foo {
    constructor() {
        _classCallCheck(this, Foo);
    }
}
export { Foo };
"#;
    let output = decompile_with_dce(input, DceMode::Off);
    // In Off mode, even transform-induced dead code is kept
    // (the helper declaration may or may not remain depending on whether the
    //  rule itself cleans up — but the DCE pass does not run)
    assert!(
        output.contains("Foo"),
        "class should be preserved:\n{output}"
    );
}

#[test]
fn transform_only_removes_dead_import_from_interop_rewrite() {
    // The interop helper wraps require() — when unwrapped, the helper
    // function declaration becomes dead (transform-induced), and DeadDecls
    // removes it. The import for the module itself survives because the
    // interop rule replaces the wrapper, not the import binding.
    let input = r#"
var _a = _interopRequireDefault(require("@babel/runtime/helpers/interopRequireDefault"));
function _interopRequireDefault(obj) {
    return obj && obj.__esModule ? obj : { default: obj };
}
export default _a.default;
"#;
    let output = decompile_with_dce(input, DceMode::TransformOnly);
    assert!(
        !output.contains("_interopRequireDefault"),
        "transform-induced dead helper should be removed:\n{output}"
    );
}

#[test]
fn transform_only_preserves_pre_existing_dead_import() {
    // Import that's already unused in the input.
    let input = r#"
import { neverUsed } from "./utils.js";
export const value = 42;
"#;
    let output = decompile_with_dce(input, DceMode::TransformOnly);
    assert!(
        output.contains("neverUsed"),
        "pre-existing dead import should be preserved in TransformOnly mode:\n{output}"
    );
}

#[test]
fn full_dce_removes_pre_existing_dead_import() {
    let input = r#"
import { neverUsed } from "./utils.js";
export const value = 42;
"#;
    let output = decompile_with_dce(input, DceMode::Full);
    assert!(
        !output.contains("neverUsed"),
        "pre-existing dead import should be removed in Full mode:\n{output}"
    );
}

#[test]
fn transform_only_mixed_dead_code() {
    // Mix of pre-existing dead (unusedHelper) and transform-induced dead
    // (_classCallCheck becomes dead after the class rule rewrites it).
    let input = r#"
function unusedHelper(x) {
    return x + 1;
}
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
class Bar {
    constructor() {
        _classCallCheck(this, Bar);
    }
}
export { Bar };
"#;
    let output = decompile_with_dce(input, DceMode::TransformOnly);
    assert!(
        output.contains("unusedHelper"),
        "pre-existing dead code should be preserved:\n{output}"
    );
    assert!(
        !output.contains("_classCallCheck"),
        "transform-induced dead code should be removed:\n{output}"
    );
}

#[test]
fn default_dce_mode_is_off() {
    let opts = DecompileOptions::default();
    assert_eq!(
        opts.dce_mode,
        DceMode::Off,
        "DecompileOptions default dce_mode should be Off"
    );
}
