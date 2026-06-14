//! Dead helper-module elimination. When a self-contained, pure helper module
//! (e.g. Babel's `_extends`) lives in its own bundle module and every consumer
//! rewrites away its usage, the consumer's binding import is downgraded to a
//! side-effect import `import "./helper.js";`. The helper module then has zero
//! binding importers and is safe to drop, along with the now-vacuous side-effect
//! imports in its consumers.

use wakaru_core::{unpack_files, DecompileOptions, RewriteLevel, UnpackInput};

/// A self-contained `_extends` helper module + a consumer that uses it in a
/// spread-rewritable call. After the pipeline the consumer's helper call becomes
/// object spread and its import is downgraded to a side-effect import.
fn helper_and_consumer() -> Vec<UnpackInput> {
    vec![
        UnpackInput {
            filename: "helper.js".to_string(),
            source: r#"
function _extends() {
    _extends = Object.assign || function(target) {
        for (var i = 1; i < arguments.length; i++) {
            var source = arguments[i];
            for (var key in source) {
                if (Object.prototype.hasOwnProperty.call(source, key)) {
                    target[key] = source[key];
                }
            }
        }
        return target;
    };
    return _extends.apply(this, arguments);
}
export default _extends;
"#
            .to_string(),
        },
        UnpackInput {
            filename: "consumer.js".to_string(),
            source: r#"import _extends from "./helper.js";
export const x = _extends({}, app_info, base_info);
"#
            .to_string(),
        },
    ]
}

// Dead-module elimination is itself dead-code cleanup: the binding->side-effect
// import downgrade it relies on only happens when `dead_code_elimination` is on.
fn dce_options() -> DecompileOptions {
    DecompileOptions {
        dead_code_elimination: true,
        ..Default::default()
    }
}

#[test]
fn drops_pure_helper_module_with_no_binding_importers() {
    let output = unpack_files(helper_and_consumer(), dce_options()).expect("unpack");
    let names: Vec<&str> = output.modules.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        !names.contains(&"helper.js"),
        "a pure helper module with no binding importers should be dropped, got {names:?}"
    );
}

#[test]
fn strips_side_effect_import_of_dropped_helper() {
    let output = unpack_files(helper_and_consumer(), dce_options()).expect("unpack");
    let consumer = output
        .modules
        .iter()
        .find(|(n, _)| n == "consumer.js")
        .map(|(_, code)| code)
        .expect("consumer module should exist");
    assert!(
        consumer.contains("...app_info"),
        "the helper call should be recovered as object spread:\n{consumer}"
    );
    assert!(
        !consumer.contains("helper.js"),
        "the vacuous side-effect import of the dropped helper should be stripped:\n{consumer}"
    );
}

/// `_objectSpread2` imports `_defineProperty` (a helper-dependency module). When
/// the consumer's usage is rewritten to spread, transitive purity + cascade
/// should drop the whole chain.
fn object_spread_chain() -> Vec<UnpackInput> {
    vec![
        UnpackInput {
            filename: "defineProperty.js".to_string(),
            source: r#"
function _defineProperty(obj, key, value) {
    if (key in obj) {
        Object.defineProperty(obj, key, { value: value, enumerable: true, configurable: true, writable: true });
    } else {
        obj[key] = value;
    }
    return obj;
}
export default _defineProperty;
"#
            .to_string(),
        },
        UnpackInput {
            filename: "objectSpread2.js".to_string(),
            source: r#"
import defineProperty from "./defineProperty.js";
function ownKeys(e, r) {
    var t = Object.keys(e);
    if (Object.getOwnPropertySymbols) {
        var o = Object.getOwnPropertySymbols(e);
        r && (o = o.filter(function (r) { return Object.getOwnPropertyDescriptor(e, r).enumerable; })), t.push.apply(t, o);
    }
    return t;
}
function _objectSpread2(e) {
    for (var r = 1; r < arguments.length; r++) {
        var t = null != arguments[r] ? arguments[r] : {};
        r % 2 ? ownKeys(Object(t), !0).forEach(function (r) { defineProperty(e, r, t[r]); }) : Object.getOwnPropertyDescriptors ? Object.defineProperties(e, Object.getOwnPropertyDescriptors(t)) : ownKeys(Object(t)).forEach(function (r) { Object.defineProperty(e, r, Object.getOwnPropertyDescriptor(t, r)); });
    }
    return e;
}
export default _objectSpread2;
"#
            .to_string(),
        },
        UnpackInput {
            filename: "consumer.js".to_string(),
            source: r#"import _objectSpread2 from "./objectSpread2.js";
export const x = _objectSpread2({}, base);
"#
            .to_string(),
        },
    ]
}

#[test]
fn drops_helper_dependency_chain_via_transitive_purity() {
    let output = unpack_files(object_spread_chain(), dce_options()).expect("unpack");
    let names: Vec<&str> = output.modules.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        !names.contains(&"objectSpread2.js") && !names.contains(&"defineProperty.js"),
        "both the helper and its pure helper-dependency should be dropped, got {names:?}"
    );
    let consumer = output
        .modules
        .iter()
        .find(|(n, _)| n == "consumer.js")
        .map(|(_, code)| code)
        .expect("consumer should remain");
    assert!(
        consumer.contains("...base") && !consumer.contains("objectSpread2"),
        "consumer should keep the spread and drop the side-effect import:\n{consumer}"
    );
}

#[test]
fn minimal_level_keeps_helper_module() {
    let output = unpack_files(
        helper_and_consumer(),
        DecompileOptions {
            dead_code_elimination: true,
            level: RewriteLevel::Minimal,
            ..Default::default()
        },
    )
    .expect("unpack");
    let names: Vec<&str> = output.modules.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.contains(&"helper.js"),
        "minimal level should not drop modules, got {names:?}"
    );
}
