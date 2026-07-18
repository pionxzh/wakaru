use wakaru_core::{is_detected_unpack_input, unpack, unpack_raw, BundleFormat, DecompileOptions};

fn cocos_bundle() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/bundles/cocos-creator-gen/dist/project.js"
    ))
    .expect("failed to read generated Cocos Creator fixture")
}

fn minified_cocos_bundle() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/bundles/cocos-creator-gen/dist/project.min.js"
    ))
    .expect("failed to read generated minified Cocos Creator fixture")
}

fn raw_output() -> wakaru_core::UnpackOutput {
    let source = cocos_bundle();
    unpack_raw(
        &source,
        &DecompileOptions {
            filename: "project.js".to_string(),
            ..Default::default()
        },
    )
    .expect("Cocos Creator raw unpack should succeed")
}

#[test]
fn cocos_creator_2x_extracts_named_modules_and_entries() {
    let output = raw_output();
    assert_eq!(output.detected_formats, vec![BundleFormat::Browserify]);

    let names: Vec<&str> = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "UIBase.js",
            "SampleActivityBase.js",
            "SampleActivityBinder.js"
        ]
    );
    assert!(
        output
            .provenance
            .iter()
            .all(|provenance| !provenance.ranges.is_empty()),
        "all extracted Cocos factories should retain source provenance"
    );

    let detected = wakaru_core::unpacker::browserify::detect_and_extract(&cocos_bundle())
        .expect("Cocos fixture should be detected");
    assert!(
        detected
            .modules
            .iter()
            .any(|module| module.id == "SampleActivityBinder" && module.is_entry),
        "Cocos entry-array membership should be retained"
    );

    let binder = output
        .modules
        .iter()
        .find(|(name, _)| name == "SampleActivityBinder.js")
        .map(|(_, code)| code)
        .expect("expected Cocos entry module");
    assert!(
        binder.contains(r#"require("./SampleActivityBase.js")"#),
        "entry dependency should target the emitted module:\n{binder}"
    );
    assert!(
        binder.contains(r#"require("cc")"#),
        "dependencies delegated to a previous Cocos bundle must remain unresolved:\n{binder}"
    );
}

#[test]
fn cocos_creator_2x_rewrites_only_factory_require_binding() {
    let output = raw_output();
    let module = output
        .modules
        .iter()
        .find(|(name, _)| name == "SampleActivityBase.js")
        .map(|(_, code)| code)
        .expect("expected Cocos module");

    assert!(
        module.contains(r#"require("./UIBase.js")"#),
        "dependency map should rewrite the factory require:\n{module}"
    );
    assert!(
        module.contains(r#"return require("../UIBase")"#),
        "shadowed inner parameter must not be rewritten:\n{module}"
    );
    assert!(
        module.contains("cc._RF.push") && module.contains("cc._RF.pop"),
        "Cocos registration side effects must be preserved:\n{module}"
    );
}

#[test]
fn cocos_creator_2x_rewrites_missing_map_request_via_basename_fallback() {
    let source = r#"
window.__require = (function() { return function() {}; })({
  UIBase: [function(require, module) {
    cc._RF.push(module, "uiBaseFixtureUuid", "UIBase");
    module.exports = class UIBase {};
    cc._RF.pop();
  }, {}],
  Feature: [function(require, module) {
    cc._RF.push(module, "featureFixtureUuid", "Feature");
    module.exports = require("../UIBase");
    cc._RF.pop();
  }, {}]
}, {}, ["Feature"]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Cocos basename-fallback bundle should unpack");
    let feature = output
        .modules
        .iter()
        .find(|(name, _)| name == "Feature.js")
        .map(|(_, code)| code)
        .expect("expected Feature module");
    assert!(
        feature.contains(r#"require("./UIBase.js")"#),
        "Cocos basename fallback should target the emitted module:\n{feature}"
    );
}

#[test]
fn cocos_creator_2x_runs_the_normal_decompile_pipeline() {
    let source = cocos_bundle();
    let output = unpack(
        &source,
        DecompileOptions {
            filename: "project.js".to_string(),
            ..Default::default()
        },
    )
    .expect("Cocos Creator unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let module = output
        .modules
        .iter()
        .find(|(name, _)| name == "SampleActivityBase.js")
        .map(|(_, code)| code)
        .expect("expected decompiled Cocos module");
    assert!(
        module.contains("import ") && module.contains(r#""./UIBase.js""#),
        "CommonJS dependency should recover as an import:\n{module}"
    );
    assert!(module.contains("cc._RF.push") && module.contains("cc._RF.pop"));
}

#[test]
fn cocos_creator_2x_detects_minified_registration_sequences() {
    let source = minified_cocos_bundle();
    assert!(
        source.contains("cc._RF.push") && source.contains("cc._RF.pop"),
        "generated minified fixture should retain Cocos registration markers"
    );
    assert!(
        source.contains("\"uiBaseFixtureUuid\",\"UIBase\"),") && source.contains(",cc._RF.pop()"),
        "generated minified fixture should combine registration calls into a comma sequence"
    );

    let output = unpack_raw(
        &source,
        &DecompileOptions {
            filename: "project.min.js".to_string(),
            ..Default::default()
        },
    )
    .expect("minified Cocos Creator bundle should unpack");
    assert_eq!(output.detected_formats, vec![BundleFormat::Browserify]);
    let names: Vec<&str> = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "UIBase.js",
            "SampleActivityBase.js",
            "SampleActivityBinder.js"
        ]
    );
}

#[test]
fn arbitrary_window_require_assignment_is_not_cocos() {
    let source = r#"
window.__require = function(modules, cache, entries) {
  return function(id) { return modules[id]; };
}({ value: [function(require, module) { module.exports = 1; }, {}] }, {}, ["value"]);
"#;

    assert!(!is_detected_unpack_input(source, false));
}

#[test]
fn nested_registration_sequences_do_not_mark_a_factory_as_cocos() {
    let source = r#"
window.__require = function(modules, cache, entries) {
  return function(id) { return modules[id]; };
}({ Fake: [function(require, module) {
  function registerLater() {
    cc._RF.push(module, "nestedFixtureUuid", "Fake"), cc._RF.pop();
  }
  module.exports = registerLater;
}, {}] }, {}, ["Fake"]);
"#;

    assert!(
        wakaru_core::unpacker::browserify::detect_and_extract(source).is_none(),
        "registration markers nested in another function are not factory-scope evidence"
    );
}

#[test]
fn cocos_creator_2x_accepts_identifier_keys_and_sanitizes_paths() {
    let source = r#"
window.__require = function(e, t, n) { return function() {}; }({
  Minified: [function(t, e, i) {
    cc._RF.push(e, "minifiedFixtureUuid", "Minified");
    i.value = 1;
    cc._RF.pop();
  }, {}],
  "../../Escape": [function(t, e, i) {
    cc._RF.push(e, "escapeFixtureUuid", "Escape");
    i.value = 2;
    cc._RF.pop();
  }, {}]
}, {}, ["Minified", "../../Escape"]);
"#;

    let result = wakaru_core::unpacker::browserify::detect_and_extract(source)
        .expect("compact Cocos bundle should be detected");
    let names: Vec<&str> = result
        .modules
        .iter()
        .map(|module| module.filename.as_str())
        .collect();
    assert_eq!(names, vec!["Minified.js", "Escape.js"]);
    assert!(result.modules.iter().all(|module| module.is_entry));
}

#[test]
fn ordinary_browserify_rejects_named_module_ids() {
    let source = r#"
(function() { return function() {}; })()({
  "main": [function(require, module) {
    module.exports = require("./escape");
  }, { "./escape": "../../Escape" }],
  "../../Escape": [function(require, module) {
    module.exports = 1;
  }, {}]
}, {}, ["main"]);
"#;

    assert!(
        wakaru_core::unpacker::browserify::detect_and_extract(source).is_none(),
        "ordinary Browserify detection should retain its numeric-ID contract"
    );
}

#[test]
fn cocos_creator_treats_numeric_property_names_as_string_ids() {
    let source = r#"
window.__require = function(e, t, n) { return function() {}; }({
  2048: [function(require, module, exports) {
    cc._RF.push(module, "numericFixtureUuid", "2048");
    module.exports = "value";
    cc._RF.pop();
  }, {}],
  Entry: [function(require, module) {
    cc._RF.push(module, "entryFixtureUuid", "Entry");
    module.exports = require("./2048");
    cc._RF.pop();
  }, { "./2048": "2048" }]
}, {}, ["Entry"]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("numeric-looking Cocos module should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    assert!(output.modules.iter().any(|(name, _)| name == "2048.js"));
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "Entry.js")
        .map(|(_, code)| code)
        .expect("named entry should exist");
    assert!(entry.contains("require(\"./2048.js\")"), "{entry}");
}
