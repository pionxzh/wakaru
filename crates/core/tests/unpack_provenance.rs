//! Provenance: every unpacked module reports the byte ranges in the original
//! bundle it was extracted from, so callers (package detection, ground-truth
//! builders) can map modules back to source-map positions.

use wakaru_core::{
    unpack, unpack_files, unpack_raw, DecompileOptions, ModuleProvenance, RewriteLevel, UnpackInput,
};

fn provenance_for<'a>(provenance: &'a [ModuleProvenance], filename: &str) -> &'a ModuleProvenance {
    provenance
        .iter()
        .find(|entry| entry.filename == filename)
        .unwrap_or_else(|| {
            panic!(
                "no provenance for {filename}; have {:?}",
                provenance.iter().map(|p| &p.filename).collect::<Vec<_>>()
            )
        })
}

fn range_text<'s>(source: &'s str, entry: &ModuleProvenance) -> Vec<&'s str> {
    entry
        .ranges
        .iter()
        .map(|&(start, end)| &source[start as usize..end as usize])
        .collect()
}

#[test]
fn esbuild_factory_modules_report_factory_decl_ranges() {
    let source = r#"
var y = (q,K)=>()=>(q&&(K=q(q=0)),K);
var mod_a = y(() => {
    mod_a_val = 42;
});
var mod_b = y(() => { mod_b_val = "hello"; });
var mod_c = y(() => { mod_c_val = true; });
var mod_d = y(() => { mod_d_val = null; });
var mod_e = y(() => { mod_e_val = undefined; });
// entry
mod_a();
mod_b();
mod_c();
mod_d();
mod_e();
console.log(mod_a_val);
"#;
    let output = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");

    let mod_a = provenance_for(&output.provenance, "mod_a.js");
    let texts = range_text(source, mod_a);
    assert_eq!(texts.len(), 1, "factory module should have one range");
    assert!(
        texts[0].starts_with("mod_a = y(") && texts[0].contains("mod_a_val = 42"),
        "range should cover the factory declarator, got: {:?}",
        texts[0]
    );

    let entry = provenance_for(&output.provenance, "entry.js");
    let entry_text = range_text(source, entry).join("\n");
    assert!(
        entry_text.contains("mod_a()") && entry_text.contains("console.log(mod_a_val)"),
        "entry ranges should cover the trailing entry statements, got: {entry_text:?}"
    );
    assert!(
        !entry_text.contains("mod_b_val = \"hello\""),
        "entry ranges should not cover factory bodies, got: {entry_text:?}"
    );
}

#[test]
fn webpack5_modules_report_factory_body_ranges() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    "./src/cjs.js": ((module) => {
      module.exports = function greet(name) {
        return "Hello, " + name;
      };
    }),
    "./src/index.js": ((__unused_webpack_module, __webpack_exports__, __webpack_require__) => {
      __webpack_require__.r(__webpack_exports__);
      var _cjs__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__("./src/cjs.js");
      console.log(_cjs__WEBPACK_IMPORTED_MODULE_0__("Ada"));
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__("./src/index.js");
})();
"#;
    let output = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");

    let cjs = provenance_for(&output.provenance, "src/cjs.js");
    let texts = range_text(source, cjs).join("\n");
    assert!(
        texts.contains("Hello, "),
        "cjs module range should cover its factory body, got: {texts:?}"
    );
    assert!(
        !texts.contains("console.log"),
        "cjs module range should not cover the index module, got: {texts:?}"
    );
}

#[test]
fn nested_webpack5_scope_split_reports_child_original_ranges() {
    let source = r#"
(self.webpackChunk_N_E = self.webpackChunk_N_E || []).push([
  [0],
  {
    100: function(module, exports, require) {
      "use strict";
      function helperA1() { return 1; }
      function helperA2() { return helperA1() + 1; }
      function helperA3() { return helperA2() * 2; }
      function helperA4() { return helperA3() + 5; }
      function publicA() { return helperA4(); }

      function helperB1() { return 10; }
      function helperB2() { return helperB1() + 10; }
      function helperB3() { return helperB2() * 20; }
      function helperB4() { return helperB3() + 50; }
      function publicB() { return helperB4(); }

      const result = publicA() + publicB();
      require.r(exports);
      require.d(exports, {
        result: function() { return result; }
      });
    }
  }
]);
"#;
    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: "chunk.js".to_string(),
            heuristic_split: true,
            level: RewriteLevel::Aggressive,
            ..Default::default()
        },
    )
    .expect("raw unpack should succeed");

    let child_entries = output
        .provenance
        .iter()
        .filter(|entry| entry.filename.starts_with("module-100/"))
        .collect::<Vec<_>>();
    assert!(
        !child_entries.is_empty(),
        "aggressive nested split should create child modules, got {:?}\nmodules: {:?}",
        output
            .provenance
            .iter()
            .map(|entry| &entry.filename)
            .collect::<Vec<_>>(),
        output.modules
    );

    let child_texts = child_entries
        .iter()
        .map(|entry| {
            (
                entry.filename.as_str(),
                range_text(source, entry).join("\n"),
            )
        })
        .collect::<Vec<_>>();
    for (filename, text) in &child_texts {
        assert!(
            !text.contains("helperA1") || !text.contains("helperB1"),
            "{filename} should not inherit the full parent factory range, got {text:?}"
        );
    }
    assert!(
        child_texts
            .iter()
            .any(|(_, text)| text.contains("helperA1") && !text.contains("helperB1")),
        "one child range should cover a nested helper family without the full parent, got {child_texts:?}"
    );
}

#[test]
fn browserify_modules_report_module_entry_ranges() {
    let source_path = "../../testcases/browserify/dist/index.js";
    let source = std::fs::read_to_string(source_path).expect("failed to read browserify testcase");
    let output = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");

    let module = output
        .provenance
        .iter()
        .find(|entry| entry.filename != "entry.js" && !entry.ranges.is_empty())
        .expect("a non-entry browserify module should have provenance ranges");
    let texts = range_text(&source, module).join("\n");
    let code = output
        .modules
        .iter()
        .find(|(name, _)| *name == module.filename)
        .map(|(_, code)| code)
        .expect("provenance filename should match an output module");
    // The range must cover the module's own factory; spot-check that a
    // distinctive token from the decompiled output appears inside the range.
    let token = code
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|tok| tok.len() >= 8)
        .find(|tok| texts.contains(tok));
    assert!(
        token.is_some(),
        "no distinctive token of {} found inside its provenance range",
        module.filename
    );
}

#[test]
fn raw_unpack_reports_provenance() {
    let source = r#"
var y = (q,K)=>()=>(q&&(K=q(q=0)),K);
var mod_a = y(() => { mod_a_val = 42; });
var mod_b = y(() => { mod_b_val = 7; });
var mod_c = y(() => { mod_c_val = 1; });
var mod_d = y(() => { mod_d_val = 2; });
var mod_e = y(() => { mod_e_val = 3; });
mod_a();
mod_b();
mod_c();
mod_d();
mod_e();
console.log(mod_a_val);
"#;
    let output = unpack_raw(source, &DecompileOptions::default()).expect("raw unpack");
    let mod_a = provenance_for(&output.provenance, "mod_a.js");
    let texts = range_text(source, mod_a);
    assert!(
        texts[0].contains("mod_a_val = 42"),
        "raw provenance should cover the factory, got: {texts:?}"
    );
}

#[test]
fn multi_source_unpack_attributes_provenance_to_inputs() {
    let chunk_a = r#"
(() => {
  var __webpack_modules__ = ({
    101: ((module) => {
      module.exports = "from chunk a";
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__(101);
})();
"#;
    let chunk_b = r#"
(() => {
  var __webpack_modules__ = ({
    202: ((module) => {
      module.exports = "from chunk b";
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__(202);
})();
"#;
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "chunk-a.js".to_string(),
                source: chunk_a.to_string(),
            },
            UnpackInput {
                filename: "chunk-b.js".to_string(),
                source: chunk_b.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("multi-source unpack should succeed");

    let module_a = output
        .provenance
        .iter()
        .find(|entry| {
            let (start, end) = match entry.ranges.first() {
                Some(&range) => range,
                None => return false,
            };
            entry.input == "chunk-a.js"
                && chunk_a[start as usize..end as usize].contains("from chunk a")
        })
        .expect("a module should be attributed to chunk-a.js with a covering range");
    assert!(
        !module_a.filename.is_empty(),
        "provenance entries must name their module file"
    );

    assert!(
        output
            .provenance
            .iter()
            .any(|entry| entry.input == "chunk-b.js"),
        "some module should be attributed to chunk-b.js"
    );
}
