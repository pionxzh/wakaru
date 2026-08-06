use std::fs;

use wakaru_core::driver::test_support::{unpack, unpack_files, unpack_files_raw, UnpackInput};
use wakaru_core::{validate_output_modules, DecompileOptions};

fn fixture(path: &str) -> String {
    let full = format!("tests/bundles/webpack-gen/dist/{path}");
    fs::read_to_string(&full).unwrap_or_else(|e| panic!("failed to read {full}: {e}"))
}

fn assert_valid_module_graph(modules: &[(String, String)]) {
    let findings = validate_output_modules(modules);
    assert!(
        findings.is_empty(),
        "unexpected graph findings: {findings:#?}"
    );
}

#[test]
fn webpack5_commonjs_chunk_unpacks_modules() {
    let source = fixture("wp5-dynamic/src_greet_js.bundle.js");
    let output = unpack(
        &source,
        DecompileOptions {
            filename: "src_greet_js.bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 CommonJS chunk should unpack");

    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    assert!(
        output
            .modules
            .iter()
            .any(|(name, code)| name == "src/greet.js" && code.contains("function greet")),
        "expected extracted src/greet.js module, got {:?}",
        output
            .modules
            .iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn webpack5_dynamic_entry_and_chunk_unpack_together() {
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "bundle.js".to_string(),
                source: fixture("wp5-dynamic/bundle.js"),
            },
            UnpackInput {
                filename: "src_greet_js.bundle.js".to_string(),
                source: fixture("wp5-dynamic/src_greet_js.bundle.js"),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("entry and chunk should unpack together");

    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let mut modules = output.modules;
    modules.sort_by(|(a, _), (b, _)| a.cmp(b));

    let names = modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"entry.js"), "missing entry.js: {names:?}");
    assert!(
        names.contains(&"src/version.js"),
        "missing entry bundle module: {names:?}"
    );
    assert!(
        names.contains(&"src/greet.js"),
        "missing chunk module: {names:?}"
    );

    let entry = modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry.js should exist");
    assert!(
        entry.contains("./src/greet.js"),
        "entry should reference the chunk module path:\n{entry}"
    );

    for (filename, code) in &modules {
        let snap_name = format!(
            "multi_file_wp5_dynamic__{}",
            filename.replace(['/', '\\'], "_").trim_end_matches(".js")
        );
        insta::assert_snapshot!(snap_name, code);
    }
}

#[test]
fn webpack5_dynamic_min_runtime_entry_and_chunk_unpack_together() {
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "bundle.js".to_string(),
                source: fixture("wp5-dynamic-min/bundle.js"),
            },
            UnpackInput {
                filename: "529.bundle.js".to_string(),
                source: fixture("wp5-dynamic-min/529.bundle.js"),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("runtime-only entry and chunk should unpack together");

    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("runtime entry should be preserved as entry.js");
    assert!(
        entry.contains("./module-529.js"),
        "entry should reference the final numeric chunk module path:\n{entry}"
    );

    assert!(
        output
            .modules
            .iter()
            .any(|(name, _)| name == "module-529.js"),
        "chunk module should be preserved: {:?}",
        output
            .modules
            .iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn webpack5_multi_file_rewrites_unambiguous_numeric_chunk_id() {
    let entry = r#"
(() => {
  var __webpack_modules__ = ({
    10: function(module, exports, __webpack_require__) {
      module.exports = "entry";
    }
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(id) {
    var cached = __webpack_module_cache__[id];
    if (cached !== undefined) return cached.exports;
    var module = __webpack_module_cache__[id] = { exports: {} };
    __webpack_modules__[id](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__.e = function(id) { return Promise.resolve(id); };
  __webpack_require__.t = function(value) { return value; };
  (() => {
    __webpack_require__.e(529).then(__webpack_require__.t.bind(__webpack_require__, 529, 19));
  })();
})();
"#;
    let chunk = r#"
exports.id = 529;
exports.ids = [529];
exports.modules = {
  529: function(module, exports) {
    exports.answer = 42;
  }
};
"#;

    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "entry.js".to_string(),
                source: entry.to_string(),
            },
            UnpackInput {
                filename: "529.bundle.js".to_string(),
                source: chunk.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("entry and numeric chunk should unpack together");

    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry.js should exist");
    assert!(
        entry.contains("./module-529.js"),
        "entry should reference the final chunk module path:\n{entry}"
    );
    assert!(
        !entry.contains(", 529,"),
        "entry should not keep the raw numeric module id:\n{entry}"
    );
}

#[test]
fn webpack5_multi_file_raw_rewrites_unambiguous_numeric_chunk_id() {
    let entry = r#"
(() => {
  var __webpack_modules__ = ({
    10: function(module, exports, __webpack_require__) {
      module.exports = "entry";
    }
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(id) {
    var cached = __webpack_module_cache__[id];
    if (cached !== undefined) return cached.exports;
    var module = __webpack_module_cache__[id] = { exports: {} };
    __webpack_modules__[id](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__.e = function(id) { return Promise.resolve(id); };
  __webpack_require__.t = function(value) { return value; };
  (() => {
    __webpack_require__.e(529).then(__webpack_require__.t.bind(__webpack_require__, 529, 19));
  })();
})();
"#;
    let chunk = r#"
exports.id = 529;
exports.ids = [529];
exports.modules = {
  529: function(module, exports) {
    exports.answer = 42;
  }
};
"#;

    let output = unpack_files_raw(
        vec![
            UnpackInput {
                filename: "entry.js".to_string(),
                source: entry.to_string(),
            },
            UnpackInput {
                filename: "529.bundle.js".to_string(),
                source: chunk.to_string(),
            },
        ],
        &DecompileOptions::default(),
    )
    .expect("raw entry and numeric chunk should unpack together");

    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry.js should exist");
    assert!(
        entry.contains("./module-529.js"),
        "raw entry should reference the final chunk module path:\n{entry}"
    );
    assert!(
        !entry.contains(", 529,"),
        "raw entry should not keep the raw numeric module id:\n{entry}"
    );
    assert!(
        !entry.contains("export "),
        "raw output should not run ESM recovery:\n{entry}"
    );
}

#[test]
fn unpack_files_restores_imported_tslib_helper_from_exported_function_provider() {
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "helpers.js".to_string(),
                source: r#"
function helper(source, excluded) {
    var target = {};
    for (var key in source) {
        if (Object.prototype.hasOwnProperty.call(source, key) && excluded.indexOf(key) < 0) {
            target[key] = source[key];
        }
    }
    if (source != null && typeof Object.getOwnPropertySymbols === "function") {
        for (var i = 0, key = Object.getOwnPropertySymbols(source); i < key.length; i++) {
            if (excluded.indexOf(key[i]) < 0 && Object.prototype.propertyIsEnumerable.call(source, key[i])) {
                target[key[i]] = source[key[i]];
            }
        }
    }
    return target;
}
export { helper as __rest };
"#
                .to_string(),
            },
            UnpackInput {
                filename: "consumer.js".to_string(),
                source: r#"
import { __rest } from "./helpers.js";
var label = props.label, rest = __rest(props, ["label"]);
use(label, rest);
"#
                .to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("plain multi-file inputs should decompile through unpack barrier");

    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let consumer = output
        .modules
        .iter()
        .find(|(name, _)| name == "consumer.js")
        .map(|(_, code)| code)
        .expect("consumer module should exist");
    assert!(
        consumer.contains("const { label, ...rest } = props;"),
        "consumer should restore imported __rest helper into object rest:\n{consumer}"
    );
    assert!(
        !consumer.contains("__rest(props"),
        "consumer should not keep the imported helper call:\n{consumer}"
    );
}

#[test]
fn webpack5_multi_file_rewrites_same_directory_dot_relative_chunk() {
    let entry = r#"
(() => {
  function require(id) { return {}; }
  require.m = {};
  require.f = {};
  require.e = function(id) { return Promise.resolve(id); };
  require.u = function(id) { return id + ".bundle.js"; };
  require.t = function(value) { return value; };
  require.e(999).then(require.t.bind(require, 999, 19));
})();
"#;
    let chunk = r#"
exports.ids = [999];
exports.modules = {
  999: function(module, exports) {
    module.exports = "same directory chunk";
  }
};
"#;

    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "entry.js".to_string(),
                source: entry.to_string(),
            },
            UnpackInput {
                filename: "./999.bundle.js".to_string(),
                source: chunk.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("same-directory dot-relative chunk should unpack with entry");

    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry.js should exist");
    assert!(
        entry.contains("./module-999.js"),
        "dot-relative chunk input should share the entry input group:\n{entry}"
    );
}

#[test]
fn webpack5_multi_file_does_not_rewrite_async_request_without_matching_chunk_id() {
    let entry = r#"
(() => {
  function require(id) { return {}; }
  require.m = {};
  require.f = {};
  require.e = function(id) { return Promise.resolve(id); };
  require.u = function(id) { return id + ".bundle.js"; };
  require.t = function(value) { return value; };
  require.e(999).then(require.t.bind(require, 999, 19));
})();
"#;
    let unrelated_chunk = r#"
exports.ids = [123];
exports.modules = {
  999: function(module, exports) {
    module.exports = "unrelated runtime";
  }
};
"#;

    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "entry.js".to_string(),
                source: entry.to_string(),
            },
            UnpackInput {
                filename: "123.bundle.js".to_string(),
                source: unrelated_chunk.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("detected unrelated webpack inputs should unpack independently");

    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry.js should exist");
    assert!(
        !entry.contains("./module-999.js"),
        "async request should not be rewritten without a matching chunk id:\n{entry}"
    );
    assert!(
        entry.contains(", 999,"),
        "async request should keep the original numeric module id:\n{entry}"
    );
}

#[test]
fn webpack5_multi_file_does_not_rewrite_matching_ids_without_chunk_filename_match() {
    let entry = r#"
(() => {
  function require(id) { return {}; }
  require.m = {};
  require.f = {};
  require.e = function(id) { return Promise.resolve(id); };
  require.u = function(id) { return id + ".bundle.js"; };
  require.t = function(value) { return value; };
  require.e(999).then(require.t.bind(require, 999, 19));
})();
"#;
    let unrelated_chunk = r#"
exports.ids = [999];
exports.modules = {
  999: function(module, exports) {
    module.exports = "unrelated runtime";
  }
};
"#;

    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "entry.js".to_string(),
                source: entry.to_string(),
            },
            UnpackInput {
                filename: "unrelated.bundle.js".to_string(),
                source: unrelated_chunk.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("detected unrelated webpack inputs should unpack independently");

    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry.js should exist");
    assert!(
        !entry.contains("./module-999.js"),
        "matching numeric ids should not rewrite without a matching chunk filename:\n{entry}"
    );
    assert!(
        entry.contains(", 999,"),
        "async request should keep the original numeric module id:\n{entry}"
    );
}

#[test]
fn webpack5_multi_file_rewrites_unambiguous_bare_require_across_inputs() {
    let entry = r#"
(() => {
  var __webpack_modules__ = ({
    20: function(module, exports, require) {
      "use strict";
      var other = require(999);
      module.exports = other;
    }
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(id) {
    var cached = __webpack_module_cache__[id];
    if (cached !== undefined) return cached.exports;
    var module = __webpack_module_cache__[id] = { exports: {} };
    __webpack_modules__[id](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__(20);
})();
"#;
    let chunk = r#"
exports.modules = {
  999: function(module, exports) {
    module.exports = "shared runtime";
  }
};
"#;

    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "entry.js".to_string(),
                source: entry.to_string(),
            },
            UnpackInput {
                filename: "shared.bundle.js".to_string(),
                source: chunk.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("inputs should unpack together");

    let module_20 = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-20.js")
        .map(|(_, code)| code)
        .expect("module-20.js should exist");
    assert!(
        module_20.contains("./module-999.js"),
        "bare numeric require should link to the unique extracted module:\n{module_20}"
    );
    assert!(
        !module_20.contains("require(999)"),
        "bare numeric require should be rewritten before UnEsm:\n{module_20}"
    );
}

#[test]
fn webpack5_multi_file_raw_rewrites_unambiguous_bare_require_across_inputs() {
    let entry = r#"
(() => {
  var __webpack_modules__ = ({
    20: function(module, exports, require) {
      "use strict";
      var other = require(999);
      module.exports = other;
    }
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(id) {
    var cached = __webpack_module_cache__[id];
    if (cached !== undefined) return cached.exports;
    var module = __webpack_module_cache__[id] = { exports: {} };
    __webpack_modules__[id](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__(20);
})();
"#;
    let chunk = r#"
exports.modules = {
  999: function(module, exports) {
    module.exports = "shared runtime";
  }
};
"#;

    let output = unpack_files_raw(
        vec![
            UnpackInput {
                filename: "entry.js".to_string(),
                source: entry.to_string(),
            },
            UnpackInput {
                filename: "shared.bundle.js".to_string(),
                source: chunk.to_string(),
            },
        ],
        &DecompileOptions::default(),
    )
    .expect("raw inputs should unpack together");

    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let module_20 = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-20.js")
        .map(|(_, code)| code)
        .expect("module-20.js should exist");
    assert!(
        module_20.contains("./module-999.js"),
        "raw bare numeric require should link to the unique extracted module:\n{module_20}"
    );
    assert!(
        !module_20.contains("require(999)"),
        "raw bare numeric require should be rewritten without running rules:\n{module_20}"
    );
    assert!(
        !module_20.contains("export "),
        "raw output should not run ESM recovery:\n{module_20}"
    );
}

#[test]
fn webpack5_multi_file_rewrites_bare_require_across_nested_chunk_directories() {
    let entry = r#"
(() => {
  var __webpack_modules__ = ({
    20: function(module, exports, require) {
      "use strict";
      var other = require(999);
      module.exports = other;
    }
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(id) {
    var cached = __webpack_module_cache__[id];
    if (cached !== undefined) return cached.exports;
    var module = __webpack_module_cache__[id] = { exports: {} };
    __webpack_modules__[id](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__(20);
})();
"#;
    let chunk = r#"
exports.modules = {
  999: function(module, exports) {
    module.exports = "shared runtime";
  }
};
"#;

    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "chunks/496.js".to_string(),
                source: entry.to_string(),
            },
            UnpackInput {
                filename: "chunks/pages/_app.js".to_string(),
                source: chunk.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("nested chunk inputs should unpack together");

    let module_20 = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-20.js")
        .map(|(_, code)| code)
        .expect("module-20.js should exist");
    assert!(
        module_20.contains("./module-999.js"),
        "bare numeric require should link to the globally unique extracted module:\n{module_20}"
    );
}

#[test]
fn webpack5_multi_file_does_not_rewrite_plain_fallback_bind_across_inputs() {
    let plain = r#"
const api = {
  t(value) {
    return value;
  },
};
const load = api.t.bind(api, 999, 19);
export { load };
"#;
    let unrelated_chunk = r#"
exports.modules = {
  999: function(module, exports) {
    module.exports = "unrelated runtime";
  }
};
"#;

    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "plain.js".to_string(),
                source: plain.to_string(),
            },
            UnpackInput {
                filename: "unrelated.bundle.js".to_string(),
                source: unrelated_chunk.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("plain fallback and unrelated chunk should unpack independently");

    let plain = output
        .modules
        .iter()
        .find(|(name, _)| name == "plain.js")
        .map(|(_, code)| code)
        .expect("plain.js should exist");
    assert!(
        !plain.contains("./module-999.js"),
        "plain fallback input should not be rewritten against an unrelated chunk:\n{plain}"
    );
    assert!(
        plain.contains("999"),
        "plain fallback input should preserve the original bind argument:\n{plain}"
    );
}

#[test]
fn webpack5_multi_file_does_not_rewrite_duplicate_numeric_ids() {
    let entry = r#"
(() => {
  var __webpack_modules__ = ({});
  var __webpack_module_cache__ = {};
  function __webpack_require__(id) {
    var cached = __webpack_module_cache__[id];
    if (cached !== undefined) return cached.exports;
    var module = __webpack_module_cache__[id] = { exports: {} };
    __webpack_modules__[id](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__.t = function(value) { return value; };
  (() => {
    const load = __webpack_require__.t.bind(__webpack_require__, 529, 19);
    load();
  })();
})();
"#;
    let chunk = r#"
exports.modules = {
  529: function(module, exports) {
    exports.answer = 42;
  }
};
"#;

    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "entry.js".to_string(),
                source: entry.to_string(),
            },
            UnpackInput {
                filename: "a.bundle.js".to_string(),
                source: chunk.to_string(),
            },
            UnpackInput {
                filename: "b.bundle.js".to_string(),
                source: chunk.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("entry and duplicate chunks should unpack together");

    let filenames = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(
        filenames.contains(&"module-529.js") && filenames.contains(&"module-529_2.js"),
        "duplicate filenames should be stabilized before facts/output: {filenames:?}"
    );

    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry.js should exist");
    assert!(
        entry.contains(", 529,"),
        "ambiguous duplicate module id should not be globally rewritten:\n{entry}"
    );
}

fn scope_bundle(offset: usize) -> String {
    format!(
        r#"
function helperA1() {{ return {offset}; }}
function helperA2() {{ return helperA1() + 1; }}
function helperA3() {{ return helperA2() * 2; }}
function helperA4() {{ return helperA3() + 3; }}
function publicA() {{ return helperA4(); }}

function helperB1() {{ return {offset}0; }}
function helperB2() {{ return helperB1() + 10; }}
function helperB3() {{ return helperB2() * 20; }}
function helperB4() {{ return helperB3() + 30; }}
function publicB() {{ return helperB4(); }}

const result = publicA() + publicB();
console.log(result);
"#
    )
}

#[test]
fn scope_hoist_processed_input_keeps_public_esm_path() {
    let target = include_str!("fixtures/public-path-facade/scope/index-hash.js");
    let consumer = include_str!("fixtures/public-path-facade/scope/consumer.js");

    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "consumer.js".to_string(),
                source: consumer.to_string(),
            },
            UnpackInput {
                filename: "index-hash.js".to_string(),
                source: target.to_string(),
            },
            UnpackInput {
                filename: "left.js".to_string(),
                source: include_str!("fixtures/public-path-facade/scope/left.js").to_string(),
            },
            UnpackInput {
                filename: "right.js".to_string(),
                source: include_str!("fixtures/public-path-facade/scope/right.js").to_string(),
            },
        ],
        DecompileOptions {
            heuristic_split: true,
            ..Default::default()
        },
    )
    .expect("scope-hoisted target should unpack with its public path");

    let facade = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "index-hash.js")
        .map(|(_, code)| code)
        .unwrap_or_else(|| {
            panic!(
                "public facade path must survive: {:?}",
                output
                    .modules
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        facade.contains("export let liveValue") && facade.contains("liveValue += 1"),
        "the facade must retain the reassigned live export:\n{facade}"
    );
    assert!(
        facade.contains("export * from \"./left.js\"")
            && facade.contains("export * from \"./right.js\""),
        "the facade must retain ambiguous star re-exports for the ESM linker:\n{facade}"
    );
    assert!(
        !facade.contains("export { helperA") && !facade.contains("export { helperB"),
        "splitter-only helper bindings must not leak through the public facade:\n{facade}"
    );
    assert!(
        output
            .modules
            .iter()
            .any(|(filename, _)| filename.starts_with("index-hash/")),
        "generated children should follow recursive split naming discipline"
    );
    assert_valid_module_graph(&output.modules);
}

#[test]
fn esbuild_processed_chunk_keeps_public_esm_path() {
    let target = include_str!("fixtures/public-path-facade/esbuild/chunk-hash.js");
    let consumer = include_str!("fixtures/public-path-facade/esbuild/consumer.js");

    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "consumer.js".to_string(),
                source: consumer.to_string(),
            },
            UnpackInput {
                filename: "chunk-hash.js".to_string(),
                source: target.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("esbuild chunk should unpack with its public path");

    assert!(output
        .detected_formats
        .contains(&wakaru_core::BundleFormat::Esbuild));
    assert!(
        output
            .modules
            .iter()
            .any(|(filename, _)| filename == "chunk-hash.js"),
        "esbuild facade path must survive: {:?}",
        output
            .modules
            .iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
    );
    assert!(
        output
            .modules
            .iter()
            .any(|(filename, _)| filename.starts_with("chunk-hash/")),
        "esbuild children should be namespaced beneath the facade"
    );
    assert_valid_module_graph(&output.modules);
}

#[test]
fn reserved_public_path_wins_generated_chunk_collision() {
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "first.js".to_string(),
                source: format!("{}\nexport {{ publicA, publicB }};", scope_bundle(1)),
            },
            UnpackInput {
                filename: "first/chunk_helperA1.js".to_string(),
                source: format!("{}\nexport {{ publicA, publicB }};", scope_bundle(2)),
            },
        ],
        DecompileOptions {
            heuristic_split: true,
            ..Default::default()
        },
    )
    .expect("reserved facade should displace the colliding generated chunk");

    let names = output
        .modules
        .iter()
        .map(|(filename, _)| filename.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"first/chunk_helperA1.js"), "{names:?}");
    assert!(names.contains(&"first/chunk_helperA1_2.js"), "{names:?}");
    let facade = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "first.js")
        .map(|(_, code)| code)
        .expect("first facade should keep its reserved path");
    assert!(
        facade.contains("./first/chunk_helperA1_2.js"),
        "facade imports must follow the deduplicated generated child:\n{facade}"
    );
    assert_valid_module_graph(&output.modules);
}

#[test]
fn duplicate_normalized_public_paths_fail_without_suffixing_facade() {
    let error = unpack_files(
        vec![
            UnpackInput {
                filename: "same.js".to_string(),
                source: scope_bundle(1),
            },
            UnpackInput {
                filename: "./same.js".to_string(),
                source: scope_bundle(2),
            },
        ],
        DecompileOptions {
            heuristic_split: true,
            ..Default::default()
        },
    )
    .expect_err("ambiguous public paths must not receive a numeric suffix");

    assert!(
        error.to_string().contains("ambiguous public module path"),
        "unexpected error: {error}"
    );
}

#[test]
fn scope_hoist_multi_file_rewrites_imports_to_deduplicated_filenames() {
    let inputs = || {
        vec![
            UnpackInput {
                filename: "first.js".to_string(),
                source: format!("{}\nexport {{ publicA, publicB }};", scope_bundle(1)),
            },
            UnpackInput {
                filename: "second.js".to_string(),
                source: format!("{}\nexport {{ publicA, publicB }};", scope_bundle(2)),
            },
        ]
    };
    let options = DecompileOptions {
        heuristic_split: true,
        ..Default::default()
    };
    let outputs = [
        (false, unpack_files(inputs(), options.clone())),
        (true, unpack_files_raw(inputs(), &options)),
    ];

    for (raw, output) in outputs {
        let output = output.expect("both scope-hoisted inputs should unpack together");
        let filenames = output
            .modules
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        let expected_filename = if raw { "entry_2.js" } else { "second.js" };
        let expected_import = if raw {
            "./chunk_helperA1_2.js"
        } else {
            "./second/chunk_helperA1.js"
        };
        let second_entry = output
            .modules
            .iter()
            .find(|(name, _)| name == expected_filename)
            .map(|(_, code)| code)
            .unwrap_or_else(|| {
                panic!(
                    "the second input should receive deduplicated module filenames: {filenames:?}"
                )
            });
        assert!(
            second_entry.contains(expected_import),
            "second input imports must follow its final sibling module:\n{second_entry}"
        );
        if raw {
            assert!(
                !second_entry.contains("./chunk_helperA1.js"),
                "raw second input must not link to the first input's colliding module:\n{second_entry}"
            );
        }
    }
}

#[test]
fn webpack5_multi_file_does_not_rewrite_duplicate_bare_require_ids() {
    let entry = r#"
(() => {
  var __webpack_modules__ = ({
    20: function(module, exports, require) {
      "use strict";
      var other = require(529);
      module.exports = other;
    }
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(id) {
    var cached = __webpack_module_cache__[id];
    if (cached !== undefined) return cached.exports;
    var module = __webpack_module_cache__[id] = { exports: {} };
    __webpack_modules__[id](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__(20);
})();
"#;
    let chunk = r#"
exports.modules = {
  529: function(module, exports) {
    exports.answer = 42;
  }
};
"#;

    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "entry.js".to_string(),
                source: entry.to_string(),
            },
            UnpackInput {
                filename: "a.bundle.js".to_string(),
                source: chunk.to_string(),
            },
            UnpackInput {
                filename: "b.bundle.js".to_string(),
                source: chunk.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("entry and duplicate chunks should unpack together");

    let module_20 = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-20.js")
        .map(|(_, code)| code)
        .expect("module-20.js should exist");
    assert!(
        !module_20.contains("./module-529.js") && !module_20.contains("./module-529_2.js"),
        "ambiguous duplicate module id should not be globally rewritten:\n{module_20}"
    );
    assert!(
        module_20.contains("require(529)"),
        "ambiguous duplicate module id should keep the numeric require:\n{module_20}"
    );
}

#[test]
fn parent_relative_inputs_keep_public_paths_without_failing() {
    // `wakaru --unpack ../pkg/*.js` is an ordinary invocation; the traversal
    // prefix cannot be mirrored under the output root, but that must not
    // abort the run — the in-bounds remainder keeps the directory structure.
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "../pkg/first.js".to_string(),
                source: scope_bundle(1),
            },
            UnpackInput {
                filename: "../pkg/second.js".to_string(),
                source: scope_bundle(2),
            },
        ],
        DecompileOptions {
            heuristic_split: true,
            ..Default::default()
        },
    )
    .expect("parent-relative CLI paths must not abort the unpack");

    let names = output
        .modules
        .iter()
        .map(|(filename, _)| filename.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"pkg/first.js"), "{names:?}");
    assert!(names.contains(&"pkg/second.js"), "{names:?}");
    assert_valid_module_graph(&output.modules);
}

#[test]
fn mixed_absolute_and_relative_inputs_keep_absolute_structure() {
    // One unrelated relative input must not collapse the absolute candidates
    // to bare basenames (which would then spuriously collide).
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "/proj/m1/widget.js".to_string(),
                source: scope_bundle(1),
            },
            UnpackInput {
                filename: "/proj/m2/widget.js".to_string(),
                source: scope_bundle(2),
            },
            UnpackInput {
                filename: "consumer.js".to_string(),
                source: "export const c = 1;\n".to_string(),
            },
        ],
        DecompileOptions {
            heuristic_split: true,
            ..Default::default()
        },
    )
    .expect("a relative sibling input must not collapse absolute candidate paths");

    let names = output
        .modules
        .iter()
        .map(|(filename, _)| filename.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"m1/widget.js"), "{names:?}");
    assert!(names.contains(&"m2/widget.js"), "{names:?}");
    assert!(names.contains(&"consumer.js"), "{names:?}");
    assert_valid_module_graph(&output.modules);
}
