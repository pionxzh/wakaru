use std::fs;

use wakaru_core::{unpack, unpack_files, unpack_files_raw, DecompileOptions, UnpackInput};

fn fixture(path: &str) -> String {
    let full = format!("tests/bundles/webpack-gen/dist/{path}");
    fs::read_to_string(&full).unwrap_or_else(|e| panic!("failed to read {full}: {e}"))
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
  function __webpack_require__(id) { return {}; }
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
  function __webpack_require__(id) { return {}; }
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
  function __webpack_require__(id) { return {}; }
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
  function __webpack_require__(id) { return {}; }
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
  function __webpack_require__(id) { return {}; }
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
  function __webpack_require__(id) { return {}; }
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
  function __webpack_require__(id) { return {}; }
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
