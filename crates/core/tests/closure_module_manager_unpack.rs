use std::fs;

use wakaru_core::{
    is_detected_unpack_input, unpack, unpack_files_raw, unpack_raw, BundleFormat, DecompileOptions,
    UnpackInput,
};

fn fixture() -> String {
    fs::read_to_string("tests/bundles/closure-module-manager/synthetic.js")
        .expect("synthetic Closure ModuleManager fixture")
}

fn annotated_served_order_fixture() -> String {
    fs::read_to_string("tests/bundles/closure-module-manager/annotated-served-order-shape.js")
        .expect("anonymized served-order fixture")
}

fn compiler_generated_fixture() -> String {
    fs::read_to_string("tests/bundles/closure-module-manager-gen/dist/compiler-chunks/bundle.js")
        .expect("Closure Compiler-generated ModuleManager fixture")
}

fn module_code<'a>(modules: &'a [(String, String)], filename: &str) -> &'a str {
    modules
        .iter()
        .find(|(name, _)| name == filename)
        .map(|(_, code)| code.as_str())
        .unwrap_or_else(|| panic!("missing {filename}; got {modules:?}"))
}

#[test]
fn annotated_wrapper_unpacks_shared_namespace_fragments_raw() {
    let source = fixture();
    let output = unpack_raw(&source, &DecompileOptions::default()).expect("raw unpack");

    assert_eq!(
        output.detected_formats,
        [BundleFormat::ClosureModuleManager]
    );
    assert_eq!(
        output
            .modules
            .iter()
            .map(|(filename, _)| filename.as_str())
            .collect::<Vec<_>>(),
        ["base.js", "feature.js", "lazy.js"]
    );

    let feature = module_code(&output.modules, "feature.js");
    assert!(
        feature.contains(".call(this, this.closureShared)"),
        "outer shared-namespace wrapper should be preserved:\n{feature}"
    );
    assert!(
        feature.starts_with("\"use strict\";\nthis.closureShared = this.closureShared || {};")
            && feature.contains("var window = this;"),
        "top-level and wrapper-local bootstrap should be preserved:\n{feature}"
    );
    assert!(
        feature.contains("shared.before(\"feature\")")
            && feature.contains("shared._DumpException(error)"),
        "loader and error guards should stay intact:\n{feature}"
    );
    assert!(
        !feature.contains("baseValue = 1") && !feature.contains("lazyValue"),
        "one output should contain one module segment:\n{feature}"
    );
    assert!(
        !feature.contains("import ") && !feature.contains("export "),
        "ModuleManager graph edges are not ESM edges:\n{feature}"
    );
}

#[test]
fn anonymized_shape_preserves_served_order_and_empty_modules() {
    let source = annotated_served_order_fixture();
    let output = unpack_raw(&source, &DecompileOptions::default()).expect("raw unpack");

    assert_eq!(
        output.detected_formats,
        [BundleFormat::ClosureModuleManager]
    );
    assert_eq!(
        output
            .modules
            .iter()
            .map(|(filename, _)| filename.as_str())
            .collect::<Vec<_>>(),
        [
            "base.js",
            "chunk_alpha.js",
            "empty_one.js",
            "empty_two.js",
            "empty_three.js",
            "chunk_beta.js",
            "empty_four.js",
            "empty_five.js",
            "chunk_final.js",
        ]
    );

    // The graph declares chunk_final second, but the fixture serves it last.
    assert!(module_code(&output.modules, "base.js").contains("_ModuleManager_initialize"));
    assert!(
        module_code(&output.modules, "chunk_alpha.js").contains("_.beginModule(\"chunk_alpha\")")
    );
    assert!(module_code(&output.modules, "chunk_beta.js").contains("fixtureComponent"));
    assert!(module_code(&output.modules, "chunk_final.js").contains("fixtureLast"));

    for empty in [
        "empty_one.js",
        "empty_two.js",
        "empty_three.js",
        "empty_four.js",
        "empty_five.js",
    ] {
        let code = module_code(&output.modules, empty);
        assert!(code.contains("this.default_SyntheticSuite"));
        assert!(code.contains("var window = this"));
        assert!(code.contains(".call(this, this.default_SyntheticSuite)"));
        assert!(!code.contains("_.beginModule(") && !code.contains("fixtureComponent"));

        let marker = format!("/*_M:{}*/", empty.trim_end_matches(".js"));
        let provenance = output
            .provenance
            .iter()
            .find(|entry| entry.filename == empty)
            .expect("empty module provenance");
        assert!(provenance
            .ranges
            .iter()
            .any(|&(start, end)| source[start as usize..end as usize] == marker));
    }

    for (_, code) in &output.modules {
        assert!(!code.contains("import ") && !code.contains("export "));
    }
}

#[test]
fn compiler_generated_chunks_unpack_in_served_order_with_empty_modules() {
    let source = compiler_generated_fixture();
    let output = unpack_raw(&source, &DecompileOptions::default()).expect("raw unpack");

    assert_eq!(
        output.detected_formats,
        [BundleFormat::ClosureModuleManager]
    );
    assert_eq!(
        output
            .modules
            .iter()
            .map(|(filename, _)| filename.as_str())
            .collect::<Vec<_>>(),
        [
            "base.js",
            "chunk_alpha.js",
            "empty_one.js",
            "empty_two.js",
            "empty_three.js",
            "chunk_beta.js",
            "empty_four.js",
            "empty_five.js",
            "chunk_final.js",
        ]
    );

    assert!(module_code(&output.modules, "base.js").contains("sampleClosureBase"));
    assert!(module_code(&output.modules, "chunk_alpha.js").contains("sampleClosureFirst"));
    assert!(module_code(&output.modules, "chunk_beta.js").contains("sampleClosureComponent"));
    assert!(module_code(&output.modules, "chunk_final.js").contains("sampleClosureLast"));

    for empty in [
        "empty_one.js",
        "empty_two.js",
        "empty_three.js",
        "empty_four.js",
        "empty_five.js",
    ] {
        let code = module_code(&output.modules, empty);
        assert!(code.contains("this.default_ClosureProducer"));
        assert!(!code.contains("sampleClosureFirst"));
        assert!(!code.contains("_.beginModule("));
    }

    for (_, code) in &output.modules {
        assert!(!code.contains("import ") && !code.contains("export "));
    }
}

#[test]
fn full_unpack_keeps_detected_modules_and_does_not_fabricate_esm() {
    let source = fixture();
    let output = unpack(
        &source,
        DecompileOptions {
            filename: "synthetic.js".to_string(),
            ..Default::default()
        },
    )
    .expect("full unpack");

    assert_eq!(
        output.detected_formats,
        [BundleFormat::ClosureModuleManager]
    );
    assert_eq!(output.modules.len(), 3);
    for (filename, code) in &output.modules {
        assert!(
            !code.contains("import ") && !code.contains("export "),
            "{filename} should remain a shared-namespace fragment:\n{code}"
        );
    }
}

#[test]
fn provenance_covers_shared_bootstrap_and_only_the_selected_segment() {
    let source = fixture();
    let output = unpack_raw(&source, &DecompileOptions::default()).expect("raw unpack");
    let provenance = output
        .provenance
        .iter()
        .find(|entry| entry.filename == "feature.js")
        .expect("feature provenance");
    let range_texts = provenance
        .ranges
        .iter()
        .map(|&(start, end)| &source[start as usize..end as usize])
        .collect::<Vec<_>>();
    assert!(range_texts.iter().any(|text| text.contains("use strict")));
    assert!(range_texts
        .iter()
        .any(|text| text.contains("this.closureShared = this.closureShared || {}")));
    assert!(range_texts
        .iter()
        .any(|text| text.contains("var window = this")));
    let extracted = range_texts
        .iter()
        .find(|text| text.starts_with("/*_M:feature*/"))
        .expect("feature segment range");
    assert!(extracted.starts_with("/*_M:feature*/"));
    assert!(extracted.contains("shared.featureValue"));
    assert!(!extracted.contains("/*_M:base*/"));
    assert!(!extracted.contains("/*_M:lazy*/"));
}

#[test]
fn graph_and_loader_boundaries_work_without_annotations() {
    let source = r#"
(function(shared) {
  try {
    shared.before("base");
    shared._ModuleManager_initialize("base/feature:0", ["base", "feature"]);
    shared.baseValue = 1;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
  try {
    shared.before("feature");
    shared.featureValue = shared.baseValue + 1;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
}).call(this, this.closureShared);
"#;
    let output = unpack_raw(source, &DecompileOptions::default()).expect("raw unpack");
    assert_eq!(
        output.detected_formats,
        [BundleFormat::ClosureModuleManager]
    );
    assert_eq!(
        output
            .modules
            .iter()
            .map(|(filename, _)| filename.as_str())
            .collect::<Vec<_>>(),
        ["base.js", "feature.js"]
    );
}

#[test]
fn callback_wrapper_and_two_argument_loader_calls_are_supported() {
    let source = r#"
loader.loaded(function(shared) {
  try {
    shared._ModuleManager_initialize("base/feature:0", ["feature"]);
    shared.runtimeReady = true;
  } catch (error) {
    shared._DumpException(error);
  }
  try {
    (0, shared.before)(shared.manager(), "feature");
    shared.featureValue = 2;
    (0, shared.after)(shared.manager(), "feature");
  } catch (error) {
    shared._DumpException(error);
  }
});
"#;
    let output = unpack_raw(source, &DecompileOptions::default()).expect("raw unpack");
    assert_eq!(
        output.detected_formats,
        [BundleFormat::ClosureModuleManager]
    );
    assert_eq!(
        output
            .modules
            .iter()
            .map(|(filename, _)| filename.as_str())
            .collect::<Vec<_>>(),
        ["base.js", "feature.js"]
    );
    let feature = module_code(&output.modules, "feature.js");
    assert!(feature.starts_with("loader.loaded(function(shared)"));
    assert!(feature.contains("shared.manager(), \"feature\""));
}

#[test]
fn bootstrap_plus_loading_ids_override_graph_declaration_order() {
    let source = r#"
(function(shared) {
  try {
    shared._ModuleManager_initialize(
      "base/late:0/feature:0",
      ["feature", "late"]
    );
    shared.baseValue = 1;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
  try {
    shared.before("feature");
    shared.featureValue = 2;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
  try {
    shared.before("late");
    shared.lateValue = 3;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
}).call(this, this.closureShared);
"#;
    let output = unpack_raw(source, &DecompileOptions::default()).expect("raw unpack");
    assert_eq!(
        output
            .modules
            .iter()
            .map(|(filename, _)| filename.as_str())
            .collect::<Vec<_>>(),
        ["base.js", "feature.js", "late.js"]
    );
    assert!(module_code(&output.modules, "feature.js").contains("featureValue"));
    assert!(module_code(&output.modules, "late.js").contains("lateValue"));
}

#[test]
fn stacked_markers_emit_empty_logical_modules() {
    let source = r#"
(function(shared) {
  /*_M:base*/
  try {
    shared._ModuleManager_initialize(
      "base/empty_a:0/empty_b:0/feature:0",
      ["empty_a", "empty_b", "feature"]
    );
    shared.baseValue = 1;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
  /*_M:empty_a*/
  /*_M:empty_b*/
  /*_M:feature*/
  try {
    shared.before("feature");
    shared.featureValue = 2;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
}).call(this, this.closureShared);
"#;
    let output = unpack_raw(source, &DecompileOptions::default()).expect("raw unpack");
    assert_eq!(
        output
            .modules
            .iter()
            .map(|(filename, _)| filename.as_str())
            .collect::<Vec<_>>(),
        ["base.js", "empty_a.js", "empty_b.js", "feature.js"]
    );
    for empty in ["empty_a.js", "empty_b.js"] {
        let code = module_code(&output.modules, empty);
        assert!(code.contains(".call(this, this.closureShared)"));
        assert!(!code.contains("featureValue") && !code.contains("baseValue"));
    }
    let empty_a = output
        .provenance
        .iter()
        .find(|entry| entry.filename == "empty_a.js")
        .expect("empty module provenance");
    assert!(empty_a
        .ranges
        .iter()
        .any(|&(start, end)| { &source[start as usize..end as usize] == "/*_M:empty_a*/" }));
}

#[test]
fn markerless_response_rejects_unrepresented_empty_modules() {
    let source = r#"
(function(shared) {
  try {
    shared._ModuleManager_initialize(
      "base/empty:0/feature:0",
      ["empty", "feature"]
    );
    shared.baseValue = 1;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
  try {
    shared.before("feature");
    shared.featureValue = 2;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
}).call(this, this.closureShared);
"#;
    assert_not_detected(source);
}

#[test]
fn annotated_module_response_detects_without_initializer() {
    let source = r#"
(function(shared) {
  /*_M:feature*/
  try {
    shared.before("feature");
    shared.featureValue = 2;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
}).call(this, this.closureShared);
"#;
    assert!(is_detected_unpack_input(source, false));
    let output = unpack_raw(source, &DecompileOptions::default()).expect("raw unpack");
    assert_eq!(
        output.detected_formats,
        [BundleFormat::ClosureModuleManager]
    );
    assert_eq!(output.modules[0].0, "feature.js");
}

#[test]
fn directory_detection_accepts_structural_fixture() {
    assert!(is_detected_unpack_input(&fixture(), false));
}

#[test]
fn multi_file_raw_attributes_provenance_and_deduplicates_names() {
    let chunk = r#"
/*_M:feature*/
try {
  shared.before("feature");
  shared.value = 1;
  shared.after();
} catch (error) {
  shared._DumpException(error);
}
"#;
    let output = unpack_files_raw(
        vec![
            UnpackInput {
                filename: "first.js".to_string(),
                source: chunk.to_string(),
            },
            UnpackInput {
                filename: "second.js".to_string(),
                source: chunk.replace("value = 1", "value = 2"),
            },
        ],
        &DecompileOptions::default(),
    )
    .expect("multi-file raw unpack");

    assert_eq!(
        output
            .modules
            .iter()
            .map(|(filename, _)| filename.as_str())
            .collect::<Vec<_>>(),
        ["feature.js", "feature_2.js"]
    );
    assert_eq!(
        output
            .provenance
            .iter()
            .map(|entry| entry.input.as_str())
            .collect::<Vec<_>>(),
        ["first.js", "second.js"]
    );
}

fn assert_not_detected(source: &str) {
    assert!(!is_detected_unpack_input(source, false));
    let output = unpack_raw(source, &DecompileOptions::default()).expect("raw fallback");
    assert!(output.detected_formats.is_empty());
    assert_eq!(output.modules[0].0, "module.js");
}

#[test]
fn rejects_unrepresented_direct_bootstrap_statement() {
    let source = r#"
shared.bootstrap = initializeRuntime();
/*_M:feature*/
try {
  shared.before("feature");
  shared.featureValue = 1;
  shared.after();
} catch (error) {
  shared._DumpException(error);
}
"#;
    assert_not_detected(source);
    let output = unpack_raw(source, &DecompileOptions::default()).expect("raw fallback");
    assert!(output.modules[0]
        .1
        .contains("shared.bootstrap = initializeRuntime()"));
}

#[test]
fn rejects_unrepresented_interstitial_wrapper_statement() {
    let source = r#"
(function(shared) {
  /*_M:base*/
  try {
    shared._ModuleManager_initialize("base/feature:0", ["feature"]);
    shared.baseValue = 1;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
  shared.bridgeValue = shared.baseValue + 1;
  /*_M:feature*/
  try {
    shared.before("feature");
    shared.featureValue = shared.bridgeValue + 1;
    shared.after();
  } catch (error) {
    shared._DumpException(error);
  }
}).call(this, this.closureShared);
"#;
    assert_not_detected(source);
    let output = unpack_raw(source, &DecompileOptions::default()).expect("raw fallback");
    assert!(output.modules[0]
        .1
        .contains("shared.bridgeValue = shared.baseValue + 1"));
}

#[test]
fn rejects_invalid_graph_indexes() {
    assert_not_detected(
        r#"
/*_M:base*/
try {
  shared.before("base");
  shared._ModuleManager_initialize("base/feature:2", ["base"]);
  shared.after();
} catch (error) {
  shared._DumpException(error);
}
"#,
    );
}

#[test]
fn rejects_forward_graph_indexes() {
    assert_not_detected(
        r#"
/*_M:base*/
try {
  shared.before("base");
  shared._ModuleManager_initialize(
    "base/feature:2/dependency",
    ["base"]
  );
  shared.after();
} catch (error) {
  shared._DumpException(error);
}
"#,
    );
}

#[test]
fn rejects_marker_without_dump_exception_guard() {
    assert_not_detected(
        r#"
/*_M:feature*/
try {
  shared.before("feature");
  shared.after();
} catch (error) {
  console.error(error);
}
"#,
    );
}

#[test]
fn rejects_marker_that_disagrees_with_loader_boundary() {
    assert_not_detected(
        r#"
/*_M:base*/
try {
  shared.before("feature");
  shared._ModuleManager_initialize("base/feature:0", ["base"]);
  shared.after();
} catch (error) {
  shared._DumpException(error);
}
"#,
    );
}

#[test]
fn marker_text_inside_a_string_is_not_a_boundary() {
    assert_not_detected(
        r#"
const marker = "/*_M:not_a_module*/";
try {
  work();
} catch (error) {
  shared._DumpException(error);
}
"#,
    );
}

#[test]
fn marker_text_embedded_inside_a_comment_is_not_a_boundary() {
    assert_not_detected(
        r#"
/* Documentation mentioning /*_M:not_a_module*/
try {
  work();
} catch (error) {
  shared._DumpException(error);
}
"#,
    );
}
