mod common;

use std::fs;

use wakaru_core::driver::test_support::{unpack, unpack_files, unpack_files_raw, UnpackInput};
use wakaru_core::driver::{prepare_unpack_input, unpack_prepared_inputs};
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
fn webpack_exports_iife_preserves_a_shared_mutable_default() {
    // A webpack-extracted UMD factory mutates the supplied exports object.
    let provider = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    100: function(module, exports) {
        !function(out) {
            "use strict";
            out.value = 42;
            out.read = function() { return out.value; };
            globalThis.published = out;
            Object.defineProperty(out, "__esModule", { value: true });
        }(exports);
    }
}]);
"#;
    let consumer = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[2], {
    200: function(module, exports, load) {
        var provider = load(100);
        module.exports = provider;
    },
    300: function(module, exports, load) {
        var value = load(100).value;
        exports.result = value;
    }
}]);
"#;
    for emit_source_map in [false, true] {
        let inputs = vec![
            UnpackInput {
                filename: "provider.js".into(),
                source: provider.into(),
            },
            UnpackInput {
                filename: "consumer.js".into(),
                source: consumer.into(),
            },
        ];
        let output = unpack_files(
            inputs.clone(),
            DecompileOptions {
                emit_source_map,
                ..Default::default()
            },
        )
        .expect("exports-object IIFE should recover");
        assert_valid_module_graph(&output.modules);
        let provider = &output
            .modules
            .iter()
            .find(|(name, _)| name == "module-100.js")
            .unwrap()
            .1;
        assert!(provider.contains("export default"), "{provider}");
        assert!(provider.contains("globalThis.published"), "{provider}");
        assert!(!provider.contains("module.exports"), "{provider}");
        let raw = unpack_files_raw(inputs, &DecompileOptions::default())
            .expect("raw extraction should stay unchanged");
        let raw_provider = &raw
            .modules
            .iter()
            .find(|(name, _)| name == "module-100.js")
            .unwrap()
            .1;
        assert!(raw_provider.contains("}(exports)"), "{raw_provider}");
        assert!(!raw_provider.contains("_webpackExports"), "{raw_provider}");
    }
}

#[test]
fn webpack_chunks_recover_healthy_edges_beside_an_opaque_factory() {
    let application = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    100: function(module, exports, load) {
        var dependency = load(200);
        exports.result = dependency.answer;
    },
    400: function(module, exports) { exports.value = 42; }
}]);
"#;
    let library = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[2], {
    200: function(module, exports, load) {
        var dependency = load(400);
        exports.answer = dependency.value;
    },
    300: function(module, exports, load) {
        function capture() { return exports; }
        exports = module.exports = load(200);
        exports.capture = capture;
    }
}]);
"#;

    for emit_source_map in [false, true] {
        let output = unpack_files(
            vec![
                UnpackInput {
                    filename: "app.js".into(),
                    source: application.into(),
                },
                UnpackInput {
                    filename: "library.js".into(),
                    source: library.into(),
                },
            ],
            DecompileOptions {
                emit_source_map,
                ..Default::default()
            },
        )
        .expect("healthy chunk edges should survive an opaque sibling");
        assert_eq!(output.warnings.len(), 1, "{:?}", output.warnings);
        assert_eq!(output.warnings[0].filename, "module-300.js");
        assert_eq!(
            output.warnings[0].kind,
            wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        );
        for (consumer, provider) in [
            ("module-100.js", "module-200.js"),
            ("module-200.js", "module-400.js"),
        ] {
            let code = &output
                .modules
                .iter()
                .find(|(name, _)| name == consumer)
                .unwrap()
                .1;
            assert!(
                code.contains(&format!("from \"./{provider}\"")),
                "{consumer}: {code}"
            );
            assert!(!code.contains("require("), "{consumer}: {code}");
        }
        assert_valid_module_graph(&output.modules);
    }
}

#[test]
fn webpack_chunks_do_not_replace_opaque_targets_with_same_numbered_modules() {
    let source = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    100: function(module, exports, load) { module.exports = load(200); },
    200: function(module, exports, load) {
        if (globalThis.useAlternate) load = globalThis.alternateLoader;
        module.exports = load;
    }
}]);
"#;
    let other = r#"
(self.webpackChunk_other = self.webpackChunk_other || []).push([[2], {
    200: function(module) { module.exports = "other"; }
}]);
"#;
    for reverse in [false, true] {
        let mut inputs = vec![
            UnpackInput {
                filename: "app.js".into(),
                source: source.into(),
            },
            UnpackInput {
                filename: "other.js".into(),
                source: other.into(),
            },
        ];
        if reverse {
            inputs.reverse();
        }
        let output = unpack_files(inputs, DecompileOptions::default()).unwrap();
        let consumer = &output
            .modules
            .iter()
            .find(|(name, _)| name == "module-100.js")
            .unwrap()
            .1;
        assert!(consumer.contains("require(200)"), "{consumer}");
        assert!(!consumer.contains("from "), "{consumer}");
        // The opaque module's own failure, and the kept call to it.
        let kinds = output
            .warnings
            .iter()
            .map(|warning| warning.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            [
                wakaru_core::UnpackWarningKind::RuntimeResidual,
                wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed,
            ],
            "{:?}",
            output.warnings
        );
        assert!(
            output.warnings[0].filename == "module-100.js"
                && output.warnings[0]
                    .message
                    .contains("`200` in an opaque module"),
            "{:?}",
            output.warnings
        );
    }
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

    common::assert_each_snapshot(
        modules.iter().map(|(filename, code)| {
            let snap_name = format!(
                "multi_file_wp5_dynamic__{}",
                filename.replace(['/', '\\'], "_").trim_end_matches(".js")
            );
            (snap_name, code)
        }),
        |name, code| insta::assert_snapshot!(name, code),
    );
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
fn webpack5_multi_file_rewrites_chunk_id_in_every_lazy_loader_form() {
    // webpack 5.111 calls the loader from an arrow, 5.107 binds the plain
    // require, and webpack 4 binds `require.t` to `null`; each needs the
    // chunk module's path, as the bound `require.t` already gets.
    for loader in [
        "() => __webpack_require__.t(529, 19)",
        "() => (__webpack_require__(529))",
        "function () { return __webpack_require__(529); }",
        "__webpack_require__.bind(__webpack_require__, 529)",
        "__webpack_require__.t.bind(null, 529, 7)",
    ] {
        let entry = WEBPACK5_LAZY_CHUNK_ENTRY.replace("LOADER", loader);
        let entry = unpack_entry_with_chunk_529(entry);
        // `require.e(529)` keeps its chunk id.
        let loader_part = entry.replace("e(529)", "");
        assert!(
            entry.contains("./module-529.js")
                && !loader_part.contains("529,")
                && !loader_part.contains("(529)"),
            "`{loader}` should load the chunk module by its path:\n{entry}"
        );
    }
}

#[test]
fn webpack5_multi_file_keeps_value_argument_of_require_t_without_load_bit() {
    // Without `mode & 1`, `require.t`'s first argument is a value, not an id.
    let entry = WEBPACK5_LAZY_CHUNK_ENTRY.replace("LOADER", "() => __webpack_require__.t(529, 2)");
    let entry = unpack_entry_with_chunk_529(entry);
    assert!(
        entry.contains("t(529, 2)") && !entry.contains("./module-529.js"),
        "a value argument must stay as written:\n{entry}"
    );
}

const WEBPACK5_LAZY_CHUNK_ENTRY: &str = r#"
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
    __webpack_require__.e(529).then(LOADER);
  })();
})();
"#;

/// The recovered `entry.js` of `entry` unpacked with chunk 529, which holds
/// module 529.
fn unpack_entry_with_chunk_529(entry: String) -> String {
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
                source: entry,
            },
            UnpackInput {
                filename: "529.bundle.js".to_string(),
                source: chunk.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("entry and numeric chunk should unpack together");
    output
        .modules
        .into_iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry.js should exist")
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
fn scope_hoist_facade_children_rebase_carried_relative_references() {
    let target = r#"
function helperA1() { return import("../shared/dynamic.js"); }
function helperA2() { return helperA1(); }
function helperA3() { return helperA2(); }
function helperA4() { return helperA3(); }
function publicA() { return helperA4(); }

function helperB1() { return require("../shared/required.js"); }
function helperB2() { return helperB1(); }
function helperB3() { return helperB2(); }
function helperB4() { return helperB3(); }
function publicB() { return helperB4(); }

console.log(publicA(), publicB());
export { publicA, publicB };
"#;
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "consumer.js".to_string(),
                source: "import { publicA } from \"./assets/index-hash.js\"; console.log(publicA);"
                    .to_string(),
            },
            UnpackInput {
                filename: "assets/index-hash.js".to_string(),
                source: target.to_string(),
            },
            UnpackInput {
                filename: "shared/dynamic.js".to_string(),
                source: "export const dynamicValue = 1;".to_string(),
            },
            UnpackInput {
                filename: "shared/required.js".to_string(),
                source: "export default 2;".to_string(),
            },
        ],
        DecompileOptions {
            heuristic_split: true,
            ..Default::default()
        },
    )
    .expect("scope-hoisted child references should follow the facade move");

    let moved_references = output
        .modules
        .iter()
        .filter(|(filename, _)| filename.starts_with("assets/index-hash/"))
        .map(|(_, code)| code.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        moved_references.contains("../../shared/dynamic.js")
            && moved_references.contains("../../shared/required.js"),
        "carried references must remain relative to the physical input after child namespacing:\n{moved_references}"
    );
    assert_valid_module_graph(&output.modules);
}

#[test]
fn esbuild_facade_children_rebase_carried_relative_references() {
    let target = r#"
var lazy = (init, cache) => () => (init && (cache = init(init = 0)), cache);
var init_a = lazy(() => { value_a = import("../shared/dynamic.js"); });
var init_b = lazy(() => { value_b = require("../shared/required.js"); });
var init_c = lazy(() => { value_c = 3; });
var init_d = lazy(() => { value_d = 4; });
var init_e = lazy(() => { value_e = 5; });
var value_a, value_b, value_c, value_d, value_e;
init_a();
init_b();
init_c();
init_d();
init_e();
var total = value_a && value_b && value_c + value_d + value_e;
export { total };
"#;
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "consumer.js".to_string(),
                source: "import { total } from \"./assets/chunk-hash.js\"; console.log(total);"
                    .to_string(),
            },
            UnpackInput {
                filename: "assets/chunk-hash.js".to_string(),
                source: target.to_string(),
            },
            UnpackInput {
                filename: "shared/dynamic.js".to_string(),
                source: "export const dynamicValue = 1;".to_string(),
            },
            UnpackInput {
                filename: "shared/required.js".to_string(),
                source: "export default 2;".to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("esbuild child references should follow the facade move");

    assert!(
        output
            .detected_formats
            .contains(&wakaru_core::BundleFormat::Esbuild),
        "synthetic chunk must exercise structural esbuild handling"
    );
    let moved_references = output
        .modules
        .iter()
        .filter(|(filename, _)| filename.starts_with("assets/chunk-hash/"))
        .map(|(_, code)| code.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        moved_references.contains("../../shared/dynamic.js")
            && moved_references.contains("../../shared/required.js"),
        "carried references must remain relative to the physical input after child namespacing:\n{moved_references}"
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

fn unpack_named(inputs: &[(&str, &str)]) -> Vec<(String, String)> {
    unpack_files(
        inputs
            .iter()
            .map(|(filename, source)| UnpackInput {
                filename: filename.to_string(),
                source: source.to_string(),
            })
            .collect(),
        DecompileOptions::default(),
    )
    .expect("inputs should unpack together")
    .modules
}

fn module_named<'a>(modules: &'a [(String, String)], name: &str) -> &'a str {
    modules
        .iter()
        .find(|(filename, _)| filename == name)
        .map(|(_, code)| code.as_str())
        .unwrap_or_else(|| {
            panic!(
                "{name} should exist, got {:?}",
                modules.iter().map(|(name, _)| name).collect::<Vec<_>>()
            )
        })
}

const APP_CONSUMER_CHUNK: &str = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    100: function(module, exports, require) { module.exports = require(200); }
}]);
"#;

#[test]
fn numeric_require_does_not_link_into_another_builds_chunk() {
    let widget = r#"
(self.webpackChunk_widget = self.webpackChunk_widget || []).push([[9], {
    200: function(module) { module.exports = "widget"; }
}]);
"#;
    for inputs in [
        [("app.js", APP_CONSUMER_CHUNK), ("widget.js", widget)],
        [("widget.js", widget), ("app.js", APP_CONSUMER_CHUNK)],
    ] {
        let modules = unpack_named(&inputs);
        let consumer = module_named(&modules, "module-100.js");
        assert!(consumer.contains("require(200)"), "{consumer}");
        assert!(!consumer.contains("module-200"), "{consumer}");
    }
}

#[test]
fn numeric_require_from_a_chunk_does_not_link_into_a_standalone_bundle() {
    // A self-contained bundle installs no chunk-loading global: its module
    // table is private to its own runtime.
    let widget = r#"
(() => {
  var __webpack_modules__ = ({
    200: function(module) { module.exports = "widget"; },
    300: function(module, exports, require) { module.exports = require(200); }
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(id) {
    var cached = __webpack_module_cache__[id];
    if (cached !== undefined) return cached.exports;
    var module = __webpack_module_cache__[id] = { exports: {} };
    __webpack_modules__[id](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__(300);
})();
"#;
    let modules = unpack_named(&[("app.js", APP_CONSUMER_CHUNK), ("widget.js", widget)]);
    let consumer = module_named(&modules, "module-100.js");
    assert!(consumer.contains("require(200)"), "{consumer}");
    assert!(!consumer.contains("module-200"), "{consumer}");
}

#[test]
fn numeric_require_links_to_its_own_build_when_another_build_reuses_the_id() {
    let app_provider = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[2], {
    200: function(module) { module.exports = "app"; }
}]);
"#;
    let widget = r#"
(self.webpackChunk_widget = self.webpackChunk_widget || []).push([[9], {
    200: function(module) { module.exports = "widget"; }
}]);
"#;
    for inputs in [
        [
            ("app.js", APP_CONSUMER_CHUNK),
            ("widget.js", widget),
            ("app-provider.js", app_provider),
        ],
        [
            ("app-provider.js", app_provider),
            ("widget.js", widget),
            ("app.js", APP_CONSUMER_CHUNK),
        ],
    ] {
        let modules = unpack_named(&inputs);
        let (app_name, _) = modules
            .iter()
            .find(|(name, code)| name.starts_with("module-200") && code.contains("\"app\""))
            .expect("the app's module 200 should be emitted");
        let consumer = module_named(&modules, "module-100.js");
        assert!(consumer.contains(&format!("./{app_name}")), "{consumer}");
        assert!(!consumer.contains("require(200)"), "{consumer}");
    }
}

#[test]
fn numeric_require_links_a_runtime_bundle_to_chunks_of_the_global_it_binds() {
    let entry = r#"
(() => {
  var __webpack_modules__ = ({
    20: function(module, exports, require) { module.exports = require(999); }
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(id) {
    var cached = __webpack_module_cache__[id];
    if (cached !== undefined) return cached.exports;
    var module = __webpack_module_cache__[id] = { exports: {} };
    __webpack_modules__[id](module, module.exports, __webpack_require__);
    return module.exports;
  }
  var chunkLoadingGlobal = self["webpackChunk_app"] = self["webpackChunk_app"] || [];
  __webpack_require__(20);
})();
"#;
    let chunk = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[5], {
    999: function(module) { module.exports = "shared"; }
}]);
"#;
    let modules = unpack_named(&[("entry.js", entry), ("chunk.js", chunk)]);
    let consumer = module_named(&modules, "module-20.js");
    assert!(consumer.contains("./module-999.js"), "{consumer}");
    assert!(!consumer.contains("require(999)"), "{consumer}");
}

const HASHED_CONSUMER_CHUNK: &str = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    "+BL1": function(module, exports, load) {
        "use strict";
        var dep = load("NOtv");
        exports.run = function() { return dep.a(); };
    }
}]);
"#;

const HASHED_PROVIDER_CHUNK: &str = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[2], {
    "NOtv": function(module, exports) {
        "use strict";
        exports.a = function() { return 1; };
    }
}]);
"#;

#[test]
fn string_id_require_links_across_inputs_of_one_build() {
    for inputs in [
        [
            ("app.js", HASHED_CONSUMER_CHUNK),
            ("provider.js", HASHED_PROVIDER_CHUNK),
        ],
        [
            ("provider.js", HASHED_PROVIDER_CHUNK),
            ("app.js", HASHED_CONSUMER_CHUNK),
        ],
    ] {
        let modules = unpack_named(&inputs);
        let consumer = module_named(&modules, "module-+BL1.js");
        assert!(
            consumer.contains(r#"import { a } from "./module-NOtv.js""#),
            "{consumer}"
        );
        assert_valid_module_graph(&modules);
    }
}

#[test]
fn named_id_require_links_across_inputs_relative_to_the_consumer() {
    let consumer = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    "./src/app.js": function(module, exports, load) {
        "use strict";
        var dep = load("./src/util.js");
        exports.run = function() { return dep.a(); };
    }
}]);
"#;
    let provider = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[2], {
    "./src/util.js": function(module, exports) {
        "use strict";
        exports.a = function() { return 1; };
    }
}]);
"#;
    let modules = unpack_named(&[("app.js", consumer), ("provider.js", provider)]);
    let consumer = module_named(&modules, "src/app.js");
    assert!(
        consumer.contains(r#"import { a } from "./util.js""#),
        "{consumer}"
    );
    assert_valid_module_graph(&modules);
}

#[test]
fn digit_string_id_require_links_by_property_key() {
    // A hashed id can be all digits. The runtime looks the module up by
    // property key: `"2638"` is key `2638`, while `"0366"` is not key `366`.
    let consumer = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    "+BL1": function(module, exports, load) {
        module.exports = [load("2638"), load("0366")];
    }
}]);
"#;
    let provider = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[2], {
    2638: function(module) { module.exports = 1; },
    366: function(module) { module.exports = 2; }
}]);
"#;
    let modules = unpack_named(&[("app.js", consumer), ("provider.js", provider)]);
    let consumer = module_named(&modules, "module-+BL1.js");
    assert!(consumer.contains("./module-2638.js"), "{consumer}");
    assert!(!consumer.contains("./module-366.js"), "{consumer}");
}

#[test]
fn string_id_require_does_not_link_into_another_builds_chunk() {
    let widget = HASHED_PROVIDER_CHUNK.replace("webpackChunk_app", "webpackChunk_widget");
    let modules = unpack_named(&[("app.js", HASHED_CONSUMER_CHUNK), ("widget.js", &widget)]);
    let consumer = module_named(&modules, "module-+BL1.js");
    assert!(!consumer.contains("./module-NOtv"), "{consumer}");
}

#[test]
fn string_id_require_stays_unlinked_when_two_inputs_of_the_build_carry_it() {
    let modules = unpack_named(&[
        ("app.js", HASHED_CONSUMER_CHUNK),
        ("provider.js", HASHED_PROVIDER_CHUNK),
        ("copy.js", HASHED_PROVIDER_CHUNK),
    ]);
    let consumer = module_named(&modules, "module-+BL1.js");
    assert!(!consumer.contains("./module-NOtv"), "{consumer}");
}

#[test]
fn string_id_rewrite_leaves_a_free_require_of_the_same_request() {
    // `require` here is the host's, not the loader: after the loader is
    // renamed to `require` the two calls look the same, so neither links.
    let consumer = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    "+BL1": function(module, exports, load) {
        module.exports = [load("NOtv"), require("NOtv"), require("UUUU")];
    }
}]);
"#;
    let provider = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[2], {
    "NOtv": function(module) { module.exports = 1; },
    "UUUU": function(module) { module.exports = 2; }
}]);
"#;
    let modules = unpack_named(&[("app.js", consumer), ("provider.js", provider)]);
    let consumer = module_named(&modules, "module-+BL1.js");
    assert!(!consumer.contains("./module-NOtv"), "{consumer}");
    assert!(!consumer.contains("./module-UUUU"), "{consumer}");
}

#[test]
fn string_id_rewrite_leaves_a_local_reference_that_spells_another_inputs_id() {
    // The consumer's own table resolves `./src/util.js`, rewritten to
    // `./util.js` from `src/main.js`; the other input's module whose raw id
    // is `./util.js` must not capture that local reference.
    let consumer = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    "./src/main.js": function(module, exports, load) {
        "use strict";
        var dep = load("./src/util.js");
        exports.run = function() { return dep.local; };
    },
    "./src/util.js": function(module, exports) {
        "use strict";
        exports.local = 1;
    }
}]);
"#;
    let other = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[2], {
    "./util.js": function(module, exports) {
        "use strict";
        exports.other = 2;
    }
}]);
"#;
    let modules = unpack_named(&[("app.js", consumer), ("other.js", other)]);
    let consumer = module_named(&modules, "src/main.js");
    assert!(
        consumer.contains(r#"import { local } from "./util.js""#),
        "{consumer}"
    );
    assert_valid_module_graph(&modules);
}

#[test]
fn string_id_require_links_across_inputs_in_raw_mode() {
    let output = unpack_files_raw(
        vec![
            UnpackInput {
                filename: "app.js".to_string(),
                source: HASHED_CONSUMER_CHUNK.to_string(),
            },
            UnpackInput {
                filename: "provider.js".to_string(),
                source: HASHED_PROVIDER_CHUNK.to_string(),
            },
        ],
        &DecompileOptions::default(),
    )
    .expect("raw inputs should unpack together");
    let consumer = module_named(&output.modules, "module-+BL1.js");
    assert!(
        consumer.contains(r#"require("./module-NOtv.js")"#),
        "{consumer}"
    );
}

fn unpack_one(
    source: &str,
    emit_source_map: bool,
) -> wakaru_core::driver::test_support::UnpackOutput {
    unpack(
        source,
        DecompileOptions {
            filename: "chunk.js".to_string(),
            emit_source_map,
            ..Default::default()
        },
    )
    .expect("the chunk should unpack")
}

fn residual_messages(output: &wakaru_core::driver::test_support::UnpackOutput) -> Vec<&str> {
    output
        .warnings
        .iter()
        .filter(|warning| warning.kind == wakaru_core::UnpackWarningKind::RuntimeResidual)
        .map(|warning| warning.message.as_str())
        .collect()
}

#[test]
fn unresolved_string_id_stays_a_require_call_in_esm_output() {
    // As for a numeric id: the call fails in CommonJS too, so the module
    // still becomes ESM and keeps the call, which is reported.
    for emit_source_map in [false, true] {
        let output = unpack_one(HASHED_CONSUMER_CHUNK, emit_source_map);
        let consumer = module_named(&output.modules, "module-+BL1.js");
        assert!(consumer.contains(r#"require("NOtv")"#), "{consumer}");
        assert!(consumer.contains("export const run"), "{consumer}");
        assert!(!consumer.contains(r#"from "NOtv""#), "{consumer}");
        assert_eq!(
            residual_messages(&output),
            ["webpack `require` keeps module ids the split output does not link: `\"NOtv\"` in no input; these calls throw when they run"]
        );
    }
}

#[test]
fn unresolved_named_id_keeps_its_spelling() {
    let consumer = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    "./src/app.js": function(module, exports, load) {
        "use strict";
        var dep = load("./src/util.js");
        exports.run = function() { return dep.a(); };
    }
}]);
"#;
    let output = unpack_one(consumer, false);
    let consumer = module_named(&output.modules, "src/app.js");
    assert!(
        consumer.contains(r#"require("./src/util.js")"#),
        "{consumer}"
    );
    assert!(!consumer.contains("import "), "{consumer}");
    assert_eq!(residual_messages(&output).len(), 1, "{:?}", output.warnings);
}

#[test]
fn unresolved_numeric_id_is_reported() {
    let output = unpack_one(APP_CONSUMER_CHUNK, false);
    let consumer = module_named(&output.modules, "module-100.js");
    assert!(consumer.contains("require(200)"), "{consumer}");
    assert_eq!(
        residual_messages(&output),
        ["webpack `require` keeps module ids the split output does not link: `200` in no input; these calls throw when they run"]
    );
}

#[test]
fn string_id_with_two_candidates_is_reported_with_the_count() {
    let output = unpack_files(
        [
            ("app.js", HASHED_CONSUMER_CHUNK),
            ("provider.js", HASHED_PROVIDER_CHUNK),
            ("copy.js", HASHED_PROVIDER_CHUNK),
        ]
        .into_iter()
        .map(|(filename, source)| UnpackInput {
            filename: filename.to_string(),
            source: source.to_string(),
        })
        .collect(),
        DecompileOptions::default(),
    )
    .expect("inputs should unpack together");
    let consumer = module_named(&output.modules, "module-+BL1.js");
    assert!(consumer.contains(r#"require("NOtv")"#), "{consumer}");
    assert_eq!(
        residual_messages(&output),
        ["webpack `require` keeps module ids the split output does not link: `\"NOtv\"` in 2 modules of the build; these calls throw when they run"]
    );
}

#[test]
fn a_free_require_of_a_package_still_becomes_an_import_beside_a_kept_id() {
    let consumer = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    "+BL1": function(module, exports, load) {
        "use strict";
        var dep = load("NOtv"), lib = require("lodash");
        exports.run = function() { return dep.a() + lib.b(); };
    }
}]);
"#;
    let output = unpack_one(consumer, false);
    let consumer = module_named(&output.modules, "module-+BL1.js");
    assert!(consumer.contains(r#"from "lodash""#), "{consumer}");
    assert!(consumer.contains(r#"require("NOtv")"#), "{consumer}");
    assert!(!consumer.contains(r#"require("lodash")"#), "{consumer}");
}

#[test]
fn a_kept_id_behind_a_chunk_load_is_reported_once() {
    // The loader behind `require.e` is covered by the kept chunk load; the
    // one behind `Promise.resolve()` has no other report.
    let consumer = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {
    "+BL1": function(module, exports, load) {
        "use strict";
        exports.now = function() { return Promise.resolve().then(() => load("NOtv")); };
        exports.later = function() { return load.e(5).then(() => load("UUUU")); };
    }
}]);
"#;
    let output = unpack_one(consumer, false);
    let consumer = module_named(&output.modules, "module-+BL1.js");
    assert!(consumer.contains("export const now"), "{consumer}");
    assert!(consumer.contains(r#"require("UUUU")"#), "{consumer}");
    assert_eq!(
        residual_messages(&output),
        [
            "webpack `require` keeps module ids the split output does not link: `\"NOtv\"` in no input; these calls throw when they run",
            "webpack runtime member `require.e` is kept, which the split output does not define; the lazy load throws when it runs",
        ]
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
    let absolute_root = std::env::temp_dir().join("wakaru-mixed-input-root");
    let first = absolute_root.join("m1").join("widget.js");
    let second = absolute_root.join("m2").join("widget.js");
    assert!(first.is_absolute() && second.is_absolute());
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: first.to_string_lossy().into_owned(),
                source: scope_bundle(1),
            },
            UnpackInput {
                filename: second.to_string_lossy().into_owned(),
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

#[test]
fn plain_inputs_keep_directory_structure_and_sibling_imports() {
    // Flattening `sub/left.js` to its basename used to collide with the real
    // `left.js`; dedup then displaced one of them and consumer.js kept
    // importing "./left.js" — argument-order-dependent wrong-module linkage.
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "sub/left.js".to_string(),
                source: "export const otherLeft = 1;\n".to_string(),
            },
            UnpackInput {
                filename: "left.js".to_string(),
                source: "export const leftValue = 2;\n".to_string(),
            },
            UnpackInput {
                filename: "consumer.js".to_string(),
                source:
                    "import { leftValue } from \"./left.js\";\nexport const total = leftValue;\n"
                        .to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("same-basename plain inputs in different directories must coexist");

    let names = output
        .modules
        .iter()
        .map(|(filename, _)| filename.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"sub/left.js"), "{names:?}");
    assert!(names.contains(&"left.js"), "{names:?}");
    assert!(names.contains(&"consumer.js"), "{names:?}");

    let root_left = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "left.js")
        .map(|(_, code)| code)
        .expect("left.js should exist");
    assert!(
        root_left.contains("leftValue"),
        "left.js must keep its own content, not the displaced sub/left.js:\n{root_left}"
    );
    let consumer = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "consumer.js")
        .map(|(_, code)| code)
        .expect("consumer.js should exist");
    assert!(
        consumer.contains("./left.js"),
        "consumer must still resolve its sibling:\n{consumer}"
    );
    assert_valid_module_graph(&output.modules);
}

#[test]
fn cross_directory_plain_imports_keep_their_relative_layout() {
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "assets/x.js".to_string(),
                source: "import { y } from \"../lib/y.js\";\nexport const x = y + 1;\n".to_string(),
            },
            UnpackInput {
                filename: "lib/y.js".to_string(),
                source: "export const y = 1;\n".to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("cross-directory plain inputs should retain a resolvable layout");

    let names = output
        .modules
        .iter()
        .map(|(filename, _)| filename.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"assets/x.js"), "{names:?}");
    assert!(names.contains(&"lib/y.js"), "{names:?}");
    let consumer = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "assets/x.js")
        .map(|(_, code)| code)
        .expect("assets/x.js should exist");
    assert!(consumer.contains("../lib/y.js"), "{consumer}");
    assert_valid_module_graph(&output.modules);
}

#[test]
fn generated_chunk_yields_to_plain_input_public_path() {
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "first.js".to_string(),
                source: format!("{}\nexport {{ publicA, publicB }};", scope_bundle(1)),
            },
            UnpackInput {
                filename: "first/chunk_helperA1.js".to_string(),
                source: "export const authored = true;\n".to_string(),
            },
        ],
        DecompileOptions {
            heuristic_split: true,
            ..Default::default()
        },
    )
    .expect("the generated chunk should yield to the plain input path");

    let names = output
        .modules
        .iter()
        .map(|(filename, _)| filename.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"first/chunk_helperA1.js"), "{names:?}");
    assert!(names.contains(&"first/chunk_helperA1_2.js"), "{names:?}");
    let plain = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "first/chunk_helperA1.js")
        .map(|(_, code)| code)
        .expect("the authored plain input must keep its public path");
    assert!(plain.contains("authored"), "{plain}");
    let facade = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "first.js")
        .map(|(_, code)| code)
        .expect("first.js facade should exist");
    assert!(
        facade.contains("./first/chunk_helperA1_2.js"),
        "the facade must follow its renamed generated child:\n{facade}"
    );
    assert_valid_module_graph(&output.modules);
}

#[test]
fn duplicate_normalized_plain_paths_fail_without_suffixing() {
    let error = unpack_files(
        vec![
            UnpackInput {
                filename: "same.js".to_string(),
                source: "export const first = 1;\n".to_string(),
            },
            UnpackInput {
                filename: "./same.js".to_string(),
                source: "export const second = 2;\n".to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect_err("duplicate normalized plain paths must not receive a numeric suffix");

    assert!(
        error.to_string().contains("ambiguous public module path"),
        "unexpected error: {error}"
    );
}

#[test]
fn plain_and_facade_claiming_the_same_public_path_fail_closed() {
    let error = unpack_files(
        vec![
            UnpackInput {
                filename: "same.js".to_string(),
                source: format!("{}\nexport {{ publicA, publicB }};", scope_bundle(1)),
            },
            UnpackInput {
                filename: "./same.js".to_string(),
                source: "export const authored = true;\n".to_string(),
            },
        ],
        DecompileOptions {
            heuristic_split: true,
            ..Default::default()
        },
    )
    .expect_err("facade and plain identity claims must not receive numeric suffixes");

    assert!(
        error.to_string().contains("ambiguous public module path"),
        "unexpected error: {error}"
    );
}

#[test]
fn windows_separator_aliases_share_the_plain_path_collision_policy() {
    let error = unpack_files(
        vec![
            UnpackInput {
                filename: "assets\\same.js".to_string(),
                source: "export const first = 1;\n".to_string(),
            },
            UnpackInput {
                filename: "assets/same.js".to_string(),
                source: "export const second = 2;\n".to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect_err("separator aliases must collide after public-path normalization");

    assert!(
        error.to_string().contains("ambiguous public module path"),
        "unexpected error: {error}"
    );
}

#[test]
fn single_plain_prepared_input_keeps_legacy_basename() {
    let prepared = prepare_unpack_input(
        "nested/plain.js".to_string(),
        "export const value = 1;\n".to_string(),
        false,
        true,
    )
    .expect("plain input should prepare");
    let output = unpack_prepared_inputs(vec![prepared], DecompileOptions::default(), false, false)
        .expect("single plain input should unpack");

    assert_eq!(output.modules.len(), 1);
    assert_eq!(output.modules[0].filename, "plain.js");
    let modules = output
        .modules
        .iter()
        .map(|module| (module.filename.clone(), module.code.clone()))
        .collect::<Vec<_>>();
    assert_valid_module_graph(&modules);
}

#[test]
fn raw_plain_inputs_keep_the_legacy_provisional_layout() {
    let output = unpack_files_raw(
        vec![
            UnpackInput {
                filename: "assets/x.js".to_string(),
                source: "export const x = 1;\n".to_string(),
            },
            UnpackInput {
                filename: "lib/y.js".to_string(),
                source: "export const y = 2;\n".to_string(),
            },
        ],
        &DecompileOptions::default(),
    )
    .expect("raw plain inputs should retain the legacy provisional layout");

    let names = output
        .modules
        .iter()
        .map(|(filename, _)| filename.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"x.js"), "{names:?}");
    assert!(names.contains(&"y.js"), "{names:?}");
    assert!(!names.contains(&"assets/x.js"), "{names:?}");
    assert!(!names.contains(&"lib/y.js"), "{names:?}");
}

#[test]
fn script_loaded_bundle_input_path_is_not_reserved() {
    let bundle = r#"
(() => {
  var __webpack_modules__ = ({
    1: function(module) {
      module.exports = 42;
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
  __webpack_require__(1);
})();
"#;
    let output = unpack_files(
        vec![
            UnpackInput {
                // A script-loaded bundle has no ESM identity at its physical
                // filename; it must not reserve this generated module name.
                filename: "module-1.js".to_string(),
                source: bundle.to_string(),
            },
            UnpackInput {
                filename: "plain.js".to_string(),
                source: "export const plain = true;\n".to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("script-loaded bundle should not reserve its physical input path");

    assert!(
        output
            .detected_formats
            .contains(&wakaru_core::BundleFormat::Webpack5),
        "expected webpack5 detection: {:?}",
        output.detected_formats
    );
    let names = output
        .modules
        .iter()
        .map(|(filename, _)| filename.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"module-1.js"), "{names:?}");
    assert!(!names.contains(&"module-1_2.js"), "{names:?}");
    assert_valid_module_graph(&output.modules);
}

#[test]
fn parent_relative_plain_sibling_stays_next_to_candidate() {
    // A plain sibling passed via the same `..` prefix as a facade candidate
    // must keep the shared directory so its relative import still resolves.
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "../pkg/lib.js".to_string(),
                source: scope_bundle(1),
            },
            UnpackInput {
                filename: "../pkg/consumer.js".to_string(),
                source: "import \"./lib.js\";\nexport const ok = true;\n".to_string(),
            },
        ],
        DecompileOptions {
            heuristic_split: true,
            ..Default::default()
        },
    )
    .expect("parent-relative plain siblings must unpack alongside candidates");

    let names = output
        .modules
        .iter()
        .map(|(filename, _)| filename.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"pkg/lib.js"), "{names:?}");
    assert!(names.contains(&"pkg/consumer.js"), "{names:?}");
    assert_valid_module_graph(&output.modules);
}

#[test]
fn collapsed_parent_relative_path_collision_fails_closed() {
    // `../left.js` collapses to "left.js" after the traversal strip. There is
    // no sound suffix rewrite for author-written cross-input references, so
    // the two physical identity claims must fail instead of picking a winner.
    let error = unpack_files(
        vec![
            UnpackInput {
                filename: "../left.js".to_string(),
                source: "export const outer = 1;\n".to_string(),
            },
            UnpackInput {
                filename: "left.js".to_string(),
                source: "export const leftValue = 2;\n".to_string(),
            },
            UnpackInput {
                filename: "consumer.js".to_string(),
                source:
                    "import { leftValue } from \"./left.js\";\nexport const total = leftValue;\n"
                        .to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect_err("collapsed and faithful claims must not be silently suffixed");

    assert!(
        error.to_string().contains("ambiguous public module path"),
        "unexpected error: {error}"
    );
}

// tslib 2.8.1 `tslib.es6.mjs` interop helpers, as a bundler includes tslib
// as its own module.
const TSLIB_INTEROP_HELPERS: &str = r#"
export var __createBinding = Object.create ? (function(o, m, k, k2) {
  if (k2 === undefined) k2 = k;
  var desc = Object.getOwnPropertyDescriptor(m, k);
  if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
  }
  Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
  if (k2 === undefined) k2 = k;
  o[k2] = m[k];
});
var __setModuleDefault = Object.create ? (function(o, v) {
  Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
  o["default"] = v;
};
var ownKeys = function(o) {
  ownKeys = Object.getOwnPropertyNames || function (o) {
    var ar = [];
    for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
    return ar;
  };
  return ownKeys(o);
};
export function __importStar(mod) {
  if (mod && mod.__esModule) return mod;
  var result = {};
  if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
  __setModuleDefault(result, mod);
  return result;
}
export function __importDefault(mod) {
  return (mod && mod.__esModule) ? mod : { default: mod };
}
"#;

fn unpack_tslib_interop_consumer(consumer: &str, providers: &[(&str, &str)]) -> String {
    let mut inputs = vec![
        UnpackInput {
            filename: "tslib.js".to_string(),
            source: TSLIB_INTEROP_HELPERS.to_string(),
        },
        UnpackInput {
            filename: "consumer.js".to_string(),
            source: consumer.to_string(),
        },
    ];
    inputs.extend(providers.iter().map(|(filename, source)| UnpackInput {
        filename: filename.to_string(),
        source: source.to_string(),
    }));
    let output = unpack_files(inputs, DecompileOptions::default())
        .expect("plain multi-file inputs should decompile through unpack barrier");
    assert_valid_module_graph(&output.modules);
    output
        .modules
        .iter()
        .find(|(name, _)| name == "consumer.js")
        .map(|(_, code)| code.clone())
        .expect("consumer module should exist")
}

// The providers of the consumer below, as tsc 5.9 (module CommonJS,
// esModuleInterop) emits an ESM source with a default export, and an
// authored CommonJS function export.
const TSC_DEFAULT_CLASS_PROVIDER: &str = r#""use strict";
Object.defineProperty(exports, "__esModule", { value: true });
class Widget {
    constructor(name) { this.name = name; }
    label() { return "widget:" + this.name; }
}
exports.default = Widget;
"#;
const MODULE_EXPORTS_FUNCTION_PROVIDER: &str = r#"function format(value) { return "<" + String(value) + ">"; }
format.version = "format-1";
module.exports = format;
"#;
const TSC_NAMED_AND_DEFAULT_PROVIDER: &str = r#""use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.twice = twice;
function twice(n) { return n * 2; }
exports.default = "util-default";
"#;

#[test]
fn bundled_tslib_interop_helpers_become_imports() {
    // shape: producer tsc@5.9.3 module=CommonJS esModuleInterop importHelpers,
    // with tslib@2.8.1 bundled as its own module by webpack@5.111.1.
    let consumer = unpack_tslib_interop_consumer(
        r#""use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.run = run;
const tslib_1 = require("./tslib.js");
const widget_js_1 = tslib_1.__importDefault(require("./widget.js"));
const format_js_1 = tslib_1.__importDefault(require("./format.js"));
const util = tslib_1.__importStar(require("./util.js"));
function run() {
    return [new widget_js_1.default("w").label(), (0, format_js_1.default)("v"), format_js_1.default.version, util.twice(3), util.default];
}
"#,
        &[
            ("widget.js", TSC_DEFAULT_CLASS_PROVIDER),
            ("format.js", MODULE_EXPORTS_FUNCTION_PROVIDER),
            ("util.js", TSC_NAMED_AND_DEFAULT_PROVIDER),
        ],
    );

    for absent in ["require(", "__importDefault", "__importStar", ".default"] {
        assert!(
            !consumer.contains(absent),
            "consumer must not keep `{absent}`:\n{consumer}"
        );
    }
    for present in [
        r#"import widget_js_1 from "./widget.js";"#,
        r#"import format_js_1 from "./format.js";"#,
        "new widget_js_1(\"w\")",
        "format_js_1(\"v\")",
        "format_js_1.version",
    ] {
        assert!(
            consumer.contains(present),
            "consumer must contain `{present}`:\n{consumer}"
        );
    }
}

#[test]
fn bundled_tslib_import_star_of_module_exports_value_keeps_member_reads() {
    // `__importStar` of an unmarked `module.exports = value` copies the
    // value's own properties and adds `default: value`: a member read is the
    // value's property, and `.default` is the value itself.
    let consumer = unpack_tslib_interop_consumer(
        r#""use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.run = run;
const tslib_1 = require("./tslib.js");
const format = tslib_1.__importStar(require("./format.js"));
function run() {
    return [format.version, format.default("v")];
}
"#,
        &[("format.js", MODULE_EXPORTS_FUNCTION_PROVIDER)],
    );

    assert!(
        consumer.contains(r#"import format from "./format.js";"#),
        "consumer must default-import the whole value:\n{consumer}"
    );
    for present in ["format.version", "format(\"v\")"] {
        assert!(
            consumer.contains(present),
            "consumer must contain `{present}`:\n{consumer}"
        );
    }
    assert!(
        !consumer.contains("require(") && !consumer.contains("__importStar"),
        "consumer must not keep the helper call:\n{consumer}"
    );
}

#[test]
fn bundled_tslib_import_default_with_whole_value_use_stays() {
    // The helper result itself escapes: `{ default: value }` for an unmarked
    // provider, which no import binding holds.
    let consumer = unpack_tslib_interop_consumer(
        r#""use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.run = run;
const tslib_1 = require("./tslib.js");
const format_js_1 = tslib_1.__importDefault(require("./format.js"));
function run() {
    return [format_js_1, format_js_1.default("v")];
}
"#,
        &[("format.js", MODULE_EXPORTS_FUNCTION_PROVIDER)],
    );

    // The call stays; its provider is the default import of the
    // `module.exports = format` provider, the value the require returned.
    assert!(
        consumer.contains(r#"import format from "./format.js";"#),
        "{consumer}"
    );
    assert!(
        consumer.contains("__importDefault(format)"),
        "a whole-value use keeps the helper call:\n{consumer}"
    );
    assert!(!consumer.contains("require("), "{consumer}");
}

#[test]
fn lowered_import_through_an_esm_helper_input_becomes_import_call() {
    // shape: producer tsc@5.9.3 module=CommonJS esModuleInterop
    // importHelpers, next to tslib's ES module build as another input.
    // Barrier 0 reads no helper module the input wrote as ESM, so `UnEsm`
    // leaves the lowered `import()` to the Phase 2 restore, which proves
    // the helper from the provider's export facts.
    let consumer = unpack_tslib_interop_consumer(
        r#""use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.load = load;
const tslib_1 = require("./tslib.js");
const widget_js_1 = tslib_1.__importDefault(require("./widget.js"));
async function load() {
    const lazy = await Promise.resolve().then(() => tslib_1.__importStar(require("./util.js")));
    return [new widget_js_1.default("w").label(), lazy.twice(3)];
}
"#,
        &[
            ("widget.js", TSC_DEFAULT_CLASS_PROVIDER),
            ("util.js", TSC_NAMED_AND_DEFAULT_PROVIDER),
        ],
    );

    assert!(
        consumer.contains(r#"await import("./util.js")"#),
        "{consumer}"
    );
    for absent in ["require(", "__importStar", "__importDefault"] {
        assert!(
            !consumer.contains(absent),
            "consumer must not keep `{absent}`:\n{consumer}"
        );
    }
}

#[test]
fn module_recovered_only_late_keeps_commonjs_for_a_helper_require() {
    // shape: hypothetical. A require inside an IIFE keeps the consumer
    // CommonJS when `UnEsm` first runs; the late ESM recovery that ends
    // Phase 2 unwraps the IIFE and converts it. The cross-module helper
    // stages have run by then and found a CommonJS module, so no stage takes
    // the require around the bundled helper, and an ES module would throw
    // `ReferenceError` where it runs.
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "tslib.js".to_string(),
                source: TSLIB_INTEROP_HELPERS.to_string(),
            },
            UnpackInput {
                filename: "consumer.js".to_string(),
                source: r#""use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.run = run;
var beta = (function () { return require("./beta.js"); })();
var tslib_1 = require("./tslib.js");
var format_js_1 = tslib_1.__importDefault(require("./format.js"));
function run() { return [beta.value, (0, format_js_1.default)("v")]; }
"#
                .to_string(),
            },
            UnpackInput {
                filename: "beta.js".to_string(),
                source: "exports.value = 1;\n".to_string(),
            },
            UnpackInput {
                filename: "format.js".to_string(),
                source: MODULE_EXPORTS_FUNCTION_PROVIDER.to_string(),
            },
            UnpackInput {
                filename: "entry.js".to_string(),
                source: r#"var consumer = require("./consumer.js");
console.log(consumer.run());
"#
                .to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("plain multi-file inputs should decompile through unpack barrier");
    assert_valid_module_graph(&output.modules);
    let consumer = output
        .modules
        .iter()
        .find(|(name, _)| name == "consumer.js")
        .map(|(_, code)| code)
        .expect("consumer module should exist");
    assert!(
        !(consumer.contains("require(") && consumer.contains("export ")),
        "an ES module consumer must not call require:\n{consumer}"
    );
}

#[test]
fn module_recovered_only_late_with_a_lazy_load_is_predicted_as_esm() {
    // shape: hypothetical. As above, but the only require the late recovery
    // keeps is a lazy load, which the lazy-load restore takes in a CommonJS
    // module before the recovery runs. The consumer becomes ESM, and the
    // facts Phase 1 predicted for it must say so, or its importer reads it
    // as CommonJS.
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "consumer.js".to_string(),
                source: r#""use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.load = load;
var beta = (function () { return require("./beta.js"); })();
function load() { return [beta, Promise.resolve().then(() => require("./gamma.js"))]; }
"#
                .to_string(),
            },
            UnpackInput {
                filename: "beta.js".to_string(),
                source: "module.exports = 1;\n".to_string(),
            },
            UnpackInput {
                filename: "gamma.js".to_string(),
                source: "exports.value = 2;\n".to_string(),
            },
            UnpackInput {
                filename: "entry.js".to_string(),
                source: r#"var consumer = require("./consumer.js");
console.log(consumer.load());
"#
                .to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("plain multi-file inputs should decompile through unpack barrier");
    assert_valid_module_graph(&output.modules);
    let consumer = output
        .modules
        .iter()
        .find(|(name, _)| name == "consumer.js")
        .map(|(_, code)| code)
        .expect("consumer module should exist");
    assert!(
        consumer.contains("export ")
            && consumer.contains(r#"import("./gamma.js")"#)
            && !consumer.contains("require("),
        "the consumer becomes ESM with the lazy load restored:\n{consumer}"
    );
}

#[test]
fn interop_helper_without_proven_body_stays() {
    // A module that only names its export `__importDefault` proves nothing.
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "tslib.js".to_string(),
                source: "export function __importDefault(mod) { return { default: mod, wrapped: true }; }\n"
                    .to_string(),
            },
            UnpackInput {
                filename: "consumer.js".to_string(),
                source: r#""use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.run = run;
const tslib_1 = require("./tslib.js");
const widget_js_1 = tslib_1.__importDefault(require("./widget.js"));
function run() { return widget_js_1.default; }
"#
                .to_string(),
            },
            UnpackInput {
                filename: "widget.js".to_string(),
                source: TSC_DEFAULT_CLASS_PROVIDER.to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("plain multi-file inputs should decompile through unpack barrier");
    let consumer = output
        .modules
        .iter()
        .find(|(name, _)| name == "consumer.js")
        .map(|(_, code)| code)
        .expect("consumer module should exist");

    // The provider marked itself `__esModule` and became an ES module, whose
    // namespace has no marker, so no import holds the value an unproven
    // function may test: the require stays (`commonjs_require_unrecovered`).
    assert!(
        consumer.contains(r#"__importDefault(require("./widget.js"))"#),
        "an unproven helper keeps its call:\n{consumer}"
    );
}

#[test]
fn a_kept_string_id_folds_its_interop_getter_like_a_numeric_one() {
    // `UnWebpackInterop` folds the `require.n` getter around a kept call, and
    // JSX written through it is recovered, for a string id as for a number.
    let chunk = |id: &str| {
        format!(
            r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {{
    "+BL1": function(module, exports, load) {{
        "use strict";
        load.r(exports); load.d(exports, {{ view: () => view }});
        var n = load({id}), o = load.n(n);
        function view() {{ return o().createElement("div", null, "hi"); }}
    }}
}}]);
"#
        )
    };
    for (id, call) in [(r#""q1tI""#, r#"require("q1tI")"#), ("11", "require(11)")] {
        let output = unpack_one(&chunk(id), false);
        let consumer = module_named(&output.modules, "module-+BL1.js");
        assert!(consumer.contains(call), "{consumer}");
        assert!(consumer.contains("<div>hi</div>"), "{consumer}");
        assert!(!consumer.contains("__esModule"), "{consumer}");
        assert!(!consumer.contains("-1"), "{consumer}");
    }
}

/// A webpack 5 bundle whose module 100 writes two named exports, a consumer
/// module 200 written by `consumer`, and module 300 that reads both.
fn written_provider_bundle(provider: &str, consumer: &str) -> String {
    format!(
        r#"(() => {{
  var __webpack_modules__ = ({{
    100: {provider},
    200: {consumer},
    300: function(module, exports, __webpack_require__) {{
      "use strict";
      var c = __webpack_require__(200);
      var p = __webpack_require__(100);
      module.exports = {{ done: c.done, f: p.f(), g: p.g() }};
    }}
  }});
  var __webpack_module_cache__ = {{}};
  function __webpack_require__(id) {{
    var cached = __webpack_module_cache__[id];
    if (cached !== undefined) return cached.exports;
    var module = __webpack_module_cache__[id] = {{ exports: {{}} }};
    __webpack_modules__[id](module, module.exports, __webpack_require__);
    return module.exports;
  }}
  __webpack_require__.r = (e) => {{ Object.defineProperty(e, "__esModule", {{ value: true }}); }};
  __webpack_require__.d = (e, d) => {{ for (var k in d) Object.defineProperty(e, k, {{ enumerable: true, get: d[k] }}); }};
  console.log(__webpack_require__(300));
}})();
"#
    )
}

const NAMED_PROVIDER: &str = r#"function(module, exports) {
      "use strict";
      exports.f = function original() { return "original"; };
      exports.g = function g() { return "g"; };
    }"#;

fn escape_warnings(output: &wakaru_core::driver::test_support::UnpackOutput) -> Vec<&str> {
    output
        .warnings
        .iter()
        .filter(|warning| warning.kind == wakaru_core::UnpackWarningKind::CommonJsExportsEscape)
        .map(|warning| warning.filename.as_str())
        .collect()
}

/// Unpacks `consumer` beside the named-only provider on both AST paths
/// (the retained AST and the source-map rerun), which must agree.
fn unpack_written_provider(consumer: &str) -> Vec<wakaru_core::driver::test_support::UnpackOutput> {
    let source = written_provider_bundle(NAMED_PROVIDER, consumer);
    [false, true]
        .into_iter()
        .map(|emit_source_map| unpack_one(&source, emit_source_map))
        .collect()
}

#[test]
fn a_provider_its_consumer_writes_stays_commonjs() {
    // Unminified: the consumer writes through its binding, and becomes ESM
    // with a default import of the provider's `module.exports`.
    let consumer = r#"function(module, exports, __webpack_require__) {
      "use strict";
      var p = __webpack_require__(100);
      p.f = function patched() { return "patched"; };
      exports.done = true;
    }"#;
    for output in unpack_written_provider(consumer) {
        assert_valid_module_graph(&output.modules);
        let provider = module_named(&output.modules, "module-100.js");
        assert!(
            provider.contains("exports.f = function original"),
            "{provider}"
        );
        assert!(!provider.contains("export "), "{provider}");
        let consumer = module_named(&output.modules, "module-200.js");
        assert!(
            consumer.contains("import p from \"./module-100.js\""),
            "{consumer}"
        );
        assert!(consumer.contains("p.f = function patched"), "{consumer}");
        let reader = module_named(&output.modules, "module-300.js");
        assert!(
            reader.contains("import p from \"./module-100.js\""),
            "{reader}"
        );
        assert!(output.warnings.iter().all(|warning| {
            warning.kind != wakaru_core::UnpackWarningKind::MissingImportedName
                && warning.kind != wakaru_core::UnpackWarningKind::CommonJsRequireOfEsm
        }));
    }
}

#[test]
fn a_provider_written_through_an_inlined_require_stays_commonjs() {
    // Terser inlines a binding used once into the write.
    let consumer = r#"function(module, exports, __webpack_require__) {
      "use strict";
      __webpack_require__(100).f = function patched() { return "patched"; };
      exports.done = true;
    }"#;
    for output in unpack_written_provider(consumer) {
        assert_valid_module_graph(&output.modules);
        let provider = module_named(&output.modules, "module-100.js");
        assert!(
            provider.contains("exports.f = function original"),
            "{provider}"
        );
        let consumer = module_named(&output.modules, "module-200.js");
        assert!(
            consumer.contains("require(\"./module-100.js\").f = function patched"),
            "{consumer}"
        );
    }
}

#[test]
fn a_provider_a_local_function_writes_stays_commonjs() {
    let consumer = r#"function(module, exports, __webpack_require__) {
      "use strict";
      function install(m) { m.f = function patched() { return "patched"; }; }
      install(__webpack_require__(100));
      exports.done = true;
    }"#;
    for output in unpack_written_provider(consumer) {
        assert_valid_module_graph(&output.modules);
        let provider = module_named(&output.modules, "module-100.js");
        assert!(
            provider.contains("exports.f = function original"),
            "{provider}"
        );
        assert!(escape_warnings(&output).is_empty());
    }
}

#[test]
fn a_provider_its_consumers_only_read_still_becomes_esm() {
    let consumer = r#"function(module, exports, __webpack_require__) {
      "use strict";
      var p = __webpack_require__(100);
      function describe(m) { return Object.keys(m).sort().join(","); }
      exports.done = describe(p) + "|" + Object.keys(p).length;
    }"#;
    for output in unpack_written_provider(consumer) {
        assert_valid_module_graph(&output.modules);
        let provider = module_named(&output.modules, "module-100.js");
        assert!(provider.contains("export const f"), "{provider}");
        let consumer = module_named(&output.modules, "module-200.js");
        assert!(
            consumer.contains("import * as p from \"./module-100.js\""),
            "{consumer}"
        );
        assert!(escape_warnings(&output).is_empty());
    }
}

#[test]
fn a_provider_passed_to_another_modules_function_becomes_esm_with_a_warning() {
    // A callee in another module is not followed: the provider becomes ESM,
    // and the consumer is reported.
    let consumer = r#"function(module, exports, __webpack_require__) {
      "use strict";
      var installer = __webpack_require__(400);
      var p = __webpack_require__(100);
      installer.install(p);
      exports.done = true;
    },
    400: function(module, exports) {
      "use strict";
      exports.install = function install(m) { m.f = function patched() {}; };
    }"#;
    for output in unpack_written_provider(consumer) {
        let provider = module_named(&output.modules, "module-100.js");
        assert!(provider.contains("export const f"), "{provider}");
        assert_eq!(escape_warnings(&output), vec!["module-200.js"]);
    }
}

#[test]
fn a_written_webpack_harmony_provider_is_left_esm() {
    // `require.d` defines getters on the harmony exports object, so a write
    // to an export does not land in the bundle either.
    let provider = r#"function(module, __webpack_exports__, __webpack_require__) {
      "use strict";
      __webpack_require__.r(__webpack_exports__);
      __webpack_require__.d(__webpack_exports__, { f: () => f, g: () => g });
      function f() { return "original"; }
      function g() { return "g"; }
    }"#;
    let consumer = r#"function(module, exports, __webpack_require__) {
      "use strict";
      var p = __webpack_require__(100);
      p.f = function patched() { return "patched"; };
      exports.done = true;
    }"#;
    let source = written_provider_bundle(provider, consumer);
    for emit_source_map in [false, true] {
        let output = unpack_one(&source, emit_source_map);
        let provider = module_named(&output.modules, "module-100.js");
        assert!(provider.contains("export { f }"), "{provider}");
        assert!(!provider.contains("__webpack_require__"), "{provider}");
    }
}

#[test]
fn a_provider_passed_to_a_bundled_interop_helper_is_not_reported() {
    // The Babel runtime's interop helper, bundled as its own module, only
    // reads the value.
    let consumer = r#"function(module, exports, __webpack_require__) {
      "use strict";
      var interop = __webpack_require__(400);
      var p = interop(__webpack_require__(100));
      exports.done = p.default.g() === "g";
    },
    400: function(module) {
      function _interopRequireDefault(e) { return e && e.__esModule ? e : { default: e }; }
      module.exports = _interopRequireDefault, module.exports.__esModule = true, module.exports["default"] = module.exports;
    }"#;
    for output in unpack_written_provider(consumer) {
        assert!(
            escape_warnings(&output).is_empty(),
            "{:#?}",
            output.warnings
        );
    }
}

#[test]
fn a_provider_passed_to_another_export_of_a_helper_module_is_reported() {
    // Module 400 exports an interop helper, but the callee is another of its
    // exports: only a proven helper export counts as reading the value.
    let consumer = r#"function(module, exports, __webpack_require__) {
      "use strict";
      var helpers = __webpack_require__(400);
      var p = __webpack_require__(100);
      helpers.install(p);
      exports.done = helpers.interop(p).default === p;
    },
    400: function(module, exports) {
      "use strict";
      exports.interop = function (e) { return e && e.__esModule ? e : { default: e }; };
      exports.install = function (m) { m.f = function patched() {}; };
    }"#;
    for output in unpack_written_provider(consumer) {
        assert_eq!(escape_warnings(&output), vec!["module-200.js"]);
    }
}

#[test]
fn a_provider_passed_to_a_helper_lookalike_is_reported() {
    // The callee has the helper's name but not its body.
    let consumer = r#"function(module, exports, __webpack_require__) {
      "use strict";
      var interop = __webpack_require__(400);
      var p = interop(__webpack_require__(100));
      exports.done = p.default.g() === "g";
    },
    400: function(module) {
      function _interopRequireDefault(e) { e.f = function patched() {}; return { default: e }; }
      module.exports = _interopRequireDefault, module.exports.__esModule = true, module.exports["default"] = module.exports;
    }"#;
    for output in unpack_written_provider(consumer) {
        assert_eq!(
            escape_warnings(&output),
            vec!["module-200.js"],
            "{:#?}",
            output.warnings
        );
    }
}

/// A webpack 5 bundle of `modules`; its entry runs module 400's `run`.
fn helper_provider_bundle(modules: &str) -> String {
    format!(
        r#"(() => {{
  var __webpack_modules__ = ({{
{modules}
  }});
  var __webpack_module_cache__ = {{}};
  function __webpack_require__(id) {{
    var cached = __webpack_module_cache__[id];
    if (cached !== undefined) return cached.exports;
    var module = __webpack_module_cache__[id] = {{ exports: {{}} }};
    __webpack_modules__[id](module, module.exports, __webpack_require__);
    return module.exports;
  }}
  __webpack_require__.r = (e) => {{ Object.defineProperty(e, "__esModule", {{ value: true }}); }};
  __webpack_require__.d = (e, d) => {{ for (var k in d) Object.defineProperty(e, k, {{ enumerable: true, get: d[k] }}); }};
  console.log(__webpack_require__(400).run());
}})();
"#
    )
}

/// Module 200: the widget a re-exporter hands on as its default.
const WIDGET_MODULE: &str = r#"200: function(module, exports) {
      "use strict";
      Object.defineProperty(exports, "__esModule", { value: true });
      class Widget { label() { return "widget"; } }
      exports.default = Widget;
    },"#;

/// producer tsc@5.9 `importHelpers`: `export { default } from "./widget"`
/// as a getter around tslib's `__importDefault`, with tslib bundled as
/// module 100 (`helper`) and a consumer that imports the default through
/// the same helper.
fn tslib_reexport_bundle(helper: &str) -> String {
    helper_provider_bundle(&format!(
        r#"    100: {helper},
    {WIDGET_MODULE}
    300: function(module, exports, __webpack_require__) {{
      "use strict";
      Object.defineProperty(exports, "__esModule", {{ value: true }});
      exports.default = void 0;
      const tslib_1 = __webpack_require__(100);
      var widget_1 = __webpack_require__(200);
      Object.defineProperty(exports, "default", {{
        enumerable: true,
        get: function () {{ return tslib_1.__importDefault(widget_1).default; }}
      }});
    }},
    400: function(module, exports, __webpack_require__) {{
      "use strict";
      Object.defineProperty(exports, "__esModule", {{ value: true }});
      exports.run = run;
      const tslib_1 = __webpack_require__(100);
      const reexport_1 = tslib_1.__importDefault(__webpack_require__(300));
      function run() {{ return new reexport_1.default().label(); }}
    }}"#
    ))
}

/// tslib's ES module build, bundled by webpack 5.
const BUNDLED_TSLIB: &str = r#"function(module, __webpack_exports__, __webpack_require__) {
      "use strict";
      __webpack_require__.d(__webpack_exports__, { __importDefault: () => __importDefault });
      function __importDefault(mod) {
        return (mod && mod.__esModule) ? mod : { "default": mod };
      }
    }"#;

#[test]
fn a_reexport_getter_through_a_bundled_tslib_recovers_esm() {
    let source = tslib_reexport_bundle(BUNDLED_TSLIB);
    for emit_source_map in [false, true] {
        let output = unpack_one(&source, emit_source_map);
        assert_valid_module_graph(&output.modules);
        let reexport = module_named(&output.modules, "module-300.js");
        assert!(
            reexport.contains("export { default } from \"./module-200.js\""),
            "{reexport}"
        );
        assert!(!reexport.contains("defineProperty"), "{reexport}");
        let consumer = module_named(&output.modules, "module-400.js");
        assert!(
            consumer.contains("import reexport_1 from \"./module-300.js\""),
            "{consumer}"
        );
        assert!(consumer.contains("new reexport_1()"), "{consumer}");
        assert!(!consumer.contains("__importDefault"), "{consumer}");
    }
}

#[test]
fn a_terser_inlined_call_of_a_bundled_tslib_helper_recovers_esm() {
    // producer webpack@5 + terser@5: the helper is renamed, and a binding
    // used once is inlined into `require(…).__importDefault(…)`.
    let source = helper_provider_bundle(&format!(
        r#"    100: function(module, __webpack_exports__, __webpack_require__) {{
      "use strict";
      function n(e) {{ return e && e.__esModule ? e : {{ default: e }}; }}
      __webpack_require__.d(__webpack_exports__, {{ __importDefault: () => n }});
    }},
    {WIDGET_MODULE}
    300: function(module, exports, __webpack_require__) {{
      "use strict";
      Object.defineProperty(exports, "__esModule", {{ value: true }});
      exports.default = void 0;
      const n = __webpack_require__(100);
      var o = __webpack_require__(200);
      Object.defineProperty(exports, "default", {{
        enumerable: true,
        get: function () {{ return n.__importDefault(o).default; }}
      }});
    }},
    400: function(module, exports, __webpack_require__) {{
      "use strict";
      exports.run = function () {{ return new n.default().label(); }};
      const n = __webpack_require__(100).__importDefault(__webpack_require__(300));
    }}"#
    ));
    for emit_source_map in [false, true] {
        let output = unpack_one(&source, emit_source_map);
        assert_valid_module_graph(&output.modules);
        let reexport = module_named(&output.modules, "module-300.js");
        assert!(
            reexport.contains("export { default } from \"./module-200.js\""),
            "{reexport}"
        );
        let consumer = module_named(&output.modules, "module-400.js");
        assert!(
            consumer.contains("import n from \"./module-300.js\""),
            "{consumer}"
        );
        assert!(consumer.contains("new n()"), "{consumer}");
        assert!(!consumer.contains("require("), "{consumer}");
    }
}

#[test]
fn a_bundled_babel_runtime_interop_helper_proves_a_reexport_getter() {
    // producer babel@7 with @babel/plugin-transform-runtime: the helper
    // module is `module.exports = helper` with its `default` mirror, and
    // `export { default } from "./widget"` reads the wrapped require.
    let source = helper_provider_bundle(&format!(
        r#"    100: function(module) {{
      function _interopRequireDefault(e) {{ return e && e.__esModule ? e : {{ default: e }}; }}
      module.exports = _interopRequireDefault, module.exports.__esModule = true, module.exports["default"] = module.exports;
    }},
    {WIDGET_MODULE}
    300: function(module, exports, __webpack_require__) {{
      "use strict";
      var _interopRequireDefault = __webpack_require__(100);
      Object.defineProperty(exports, "__esModule", {{ value: true }});
      Object.defineProperty(exports, "default", {{
        enumerable: true,
        get: function () {{ return _widget.default; }}
      }});
      var _widget = _interopRequireDefault(__webpack_require__(200));
    }},
    400: function(module, exports, __webpack_require__) {{
      "use strict";
      var _interopRequireDefault = __webpack_require__(100);
      Object.defineProperty(exports, "__esModule", {{ value: true }});
      exports.run = run;
      var _reexport = _interopRequireDefault(__webpack_require__(300));
      function run() {{ return new _reexport.default().label(); }}
    }}"#
    ));
    for emit_source_map in [false, true] {
        let output = unpack_one(&source, emit_source_map);
        assert_valid_module_graph(&output.modules);
        let reexport = module_named(&output.modules, "module-300.js");
        assert!(
            reexport.contains("import _widget from \"./module-200.js\""),
            "{reexport}"
        );
        assert!(
            reexport.contains("export { _widget as default }"),
            "{reexport}"
        );
        assert!(!reexport.contains("require("), "{reexport}");
        let consumer = module_named(&output.modules, "module-400.js");
        assert!(consumer.contains("new _reexport()"), "{consumer}");
    }
}

/// producer @babel/runtime@7.28.6 `helpers/interopRequireWildcard.js`
/// bundled by webpack@5.111.1 (mode production, no minimizer), with a
/// consumer from @babel/plugin-transform-runtime@7.28.5 (version ^7.28.6):
/// the helper replaces `module.exports` and its own binding on its first
/// call.
#[test]
fn a_bundled_babel_runtime_wildcard_that_replaces_itself_converts_in_un_esm() {
    let source = helper_provider_bundle(
        r#"    100: function(module) {
      function _interopRequireWildcard(e, t) {
        if ("function" == typeof WeakMap) var r = new WeakMap(), n = new WeakMap();
        return (module.exports = _interopRequireWildcard = function _interopRequireWildcard(e, t) {
          if (!t && e && e.__esModule) return e;
          var o, i, f = { __proto__: null, "default": e };
          if (null === e || "object" != typeof e && "function" != typeof e) return f;
          if (o = t ? n : r) {
            if (o.has(e)) return o.get(e);
            o.set(e, f);
          }
          for (var _t in e) "default" !== _t && {}.hasOwnProperty.call(e, _t) && ((i = (o = Object.defineProperty) && Object.getOwnPropertyDescriptor(e, _t)) && (i.get || i.set) ? o(f, _t, i) : f[_t] = e[_t]);
          return f;
        }, module.exports.__esModule = true, module.exports["default"] = module.exports)(e, t);
      }
      module.exports = _interopRequireWildcard, module.exports.__esModule = true, module.exports["default"] = module.exports;
    },
    200: function(module, exports) {
      "use strict";
      Object.defineProperty(exports, "__esModule", { value: true });
      exports.default = void 0;
      exports.twice = twice;
      function twice(n) { return n * 2; }
      var _default = exports.default = "util-default";
    },
    400: function(module, exports, __webpack_require__) {
      "use strict";
      var _interopRequireWildcard = __webpack_require__(100)["default"];
      Object.defineProperty(exports, "__esModule", { value: true });
      exports.run = run;
      var util = _interopRequireWildcard(__webpack_require__(200));
      function run() { return [util.twice(3), util.default, Object.keys(util).sort().join(",")]; }
    }"#,
    );

    let report = wakaru_core::driver::explain_interop(
        &source,
        DecompileOptions {
            filename: "chunk.js".to_string(),
            ..Default::default()
        },
    )
    .expect("the chunk should unpack");
    let helper = report
        .modules
        .iter()
        .find(|module| module.filename == "module-100.js")
        .expect("the helper module");
    assert_eq!(
        helper.helper_exports,
        [
            "module.exports (wildcard interop)",
            "default (wildcard interop)"
        ]
    );
    let consumer = report
        .modules
        .iter()
        .find(|module| module.filename == "module-400.js")
        .expect("the consumer module");
    let util = consumer
        .edges
        .iter()
        .find(|edge| edge.source == "./module-200.js")
        .expect("the wrapped require");
    // `UnEsm` made the namespace import in Phase 1b; no later pass did.
    assert_eq!(util.un_esm_imports, ["namespace util"]);
    assert_eq!(util.imports, ["namespace util"]);

    for emit_source_map in [false, true] {
        let output = unpack_one(&source, emit_source_map);
        assert_valid_module_graph(&output.modules);
        let consumer = module_named(&output.modules, "module-400.js");
        assert!(
            consumer.contains("import * as util from \"./module-200.js\""),
            "{consumer}"
        );
        assert!(!consumer.contains("module-100.js"), "{consumer}");
        assert!(!consumer.contains("_interopRequireWildcard"), "{consumer}");
        assert!(!consumer.contains("require("), "{consumer}");
    }
}

#[test]
fn a_provider_that_only_looks_like_tslib_is_not_trusted() {
    // The export has the helper's name but not its body.
    let source = tslib_reexport_bundle(
        r#"function(module, __webpack_exports__, __webpack_require__) {
      "use strict";
      __webpack_require__.d(__webpack_exports__, { __importDefault: () => __importDefault });
      function __importDefault(mod) { return mod; }
    }"#,
    );
    for emit_source_map in [false, true] {
        let output = unpack_one(&source, emit_source_map);
        let reexport = module_named(&output.modules, "module-300.js");
        assert!(
            reexport.contains("return tslib_1.__importDefault(widget_1).default"),
            "{reexport}"
        );
        assert!(!reexport.contains("export "), "{reexport}");
    }
}
