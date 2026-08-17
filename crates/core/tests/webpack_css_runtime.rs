use wakaru_core::driver::test_support::{unpack, unpack_raw};
use wakaru_core::{validate_output_modules, DecompileOptions, OutputFindingKind};

fn module_code<'a>(modules: &'a [(String, String)], filename: &str) -> &'a str {
    modules
        .iter()
        .find(|(name, _)| name == filename)
        .map(|(_, code)| code.as_str())
        .unwrap_or_else(|| panic!("missing {filename}; got {modules:#?}"))
}

#[test]
fn webpack5_css_runtime_recovers_module_id_and_conditional_locals() {
    let source = r#"
(() => {
  var modules = ({
    17: ((module, exports, load) => {
      var content = load(18);
      load(19);
      content.push([module.id, "body {}", "", { version: 3 }]);
      if (content.locals) {
        module.exports = content.locals;
      }
      consume(content);
    }),
    18: ((module) => {
      module.exports = {
        locals: { button: "token" },
        push: function push() {}
      };
    }),
    19: ((module, exports, load) => {
      var content = load(18);
      content.push([module.id, "aside {}", ""]);
      consumeMetadataOnly(content);
    })
  });
  var cache = {};
  function load(moduleId) {
    var module = cache[moduleId];
    if (module !== undefined) return module.exports;
    module = cache[moduleId] = { id: moduleId, exports: {} };
    modules[moduleId](module, module.exports, load);
    return module.exports;
  }
  load(17);
})();
"#;

    for emit_source_map in [false, true] {
        let output = unpack(
            source,
            DecompileOptions {
                filename: "webpack5-css-runtime.js".to_string(),
                emit_source_map,
                ..Default::default()
            },
        )
        .expect("synthetic webpack5 CSS module should unpack");
        let css = module_code(&output.modules, "module-17.js");
        assert!(!css.contains("module.id"), "{css}");
        assert!(!css.contains("module.exports"), "{css}");
        assert!(css.contains("17,"), "{css}");
        assert!(css.contains("content.locals"), "{css}");
        assert!(css.contains("export default"), "{css}");
        let metadata_only = module_code(&output.modules, "module-19.js");
        assert!(!metadata_only.contains("module.id"), "{metadata_only}");
        assert!(metadata_only.contains("19,"), "{metadata_only}");
        assert!(!metadata_only.contains("export default"), "{metadata_only}");
        assert_eq!(validate_output_modules(&output.modules), vec![]);
        if emit_source_map {
            assert!(output
                .source_maps
                .iter()
                .any(|(filename, _)| filename == "module-17.js"));
        }
    }

    let raw = unpack_raw(source, &DecompileOptions::default())
        .expect("raw splitter passthrough should still unpack");
    let css = module_code(&raw.modules, "module-17.js");
    assert!(css.contains("module.id"), "{css}");
    assert!(css.contains("module.exports"), "{css}");
}

#[test]
fn legacy_webpack_jsonp_css_runtime_recovers_module_i() {
    let source = r#"
(window.webpackJsonp = window.webpackJsonp || []).push([[7], {
  21: function(module, exports, load) {
    var content = load(22);
    if ((content = typeof content === "string"
      ? [[module.i, content, ""]]
      : content).locals) {
      module.exports = content.locals;
    }
    inject(content);
  },
  22: function(module) {
    module.exports = "body {}";
  }
}]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "legacy-css-chunk.js".to_string(),
            ..Default::default()
        },
    )
    .expect("synthetic legacy webpack chunk should unpack");
    let css = module_code(&output.modules, "module-21.js");
    assert!(!css.contains("module.i"), "{css}");
    assert!(!css.contains("module.exports"), "{css}");
    assert!(css.contains("21,"), "{css}");
    assert!(css.contains("content.locals"), "{css}");
    assert!(css.contains("export default"), "{css}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn modern_webpack_chunk_does_not_guess_that_module_i_is_the_runtime_id() {
    let source = r#"
(self.webpackChunk_demo = self.webpackChunk_demo || []).push([[7], {
  24: ((module, exports, load) => {
    const content = load(25);
    content.push([module.i, "body {}", ""]);
  }),
  25: ((module) => {
    module.exports = { push: function push() {} };
  })
}]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "modern-css-chunk.js".to_string(),
            ..Default::default()
        },
    )
    .expect("synthetic modern webpack chunk should still unpack");
    let css = module_code(&output.modules, "module-24.js");
    assert!(css.contains("module.i"), "{css}");
    assert!(validate_output_modules(&output.modules)
        .iter()
        .any(|finding| finding.kind == OutputFindingKind::EsmCommonJsResidual));
}

#[test]
fn quoted_numeric_webpack_id_does_not_invent_a_numeric_runtime_value() {
    let source = r#"
(self.webpackChunk_demo = self.webpackChunk_demo || []).push([[4], {
  "23": ((module, exports, load) => {
    const content = load("dep");
    content.push([module.id, "body {}", ""]);
  }),
  dep: ((module) => {
    module.exports = { push: function push() {} };
  })
}]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "quoted-id-css-chunk.js".to_string(),
            ..Default::default()
        },
    )
    .expect("quoted-id webpack chunk should still unpack");
    let css = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("module.id"))
        .map(|(_, code)| code)
        .expect("unproven runtime id should remain visible");
    assert!(css.contains("module.id"), "{css}");
    assert!(validate_output_modules(&output.modules)
        .iter()
        .any(|finding| finding.kind == OutputFindingKind::EsmCommonJsResidual));
}
