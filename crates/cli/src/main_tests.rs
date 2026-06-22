use super::*;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn parses_extract_without_js_input() {
    let cli = Cli::try_parse_from(["wakaru", "extract", "input.js.map", "-o", "src"])
        .expect("extract command should parse");

    match cli.command {
        Some(Command::Extract(args)) => {
            assert_eq!(args.map, PathBuf::from("input.js.map"));
            assert_eq!(args.output, PathBuf::from("src"));
        }
        other => panic!("expected extract command, got {other:?}"),
    }
}

#[test]
fn rejects_legacy_extract_flag() {
    assert!(Cli::try_parse_from([
        "wakaru",
        "input.js",
        "--extract",
        "-m",
        "input.js.map",
        "-o",
        "src"
    ])
    .is_err());
}

#[test]
fn parses_debug_trace_command() {
    let cli = Cli::try_parse_from([
        "wakaru",
        "debug",
        "trace",
        "input.js",
        "--from",
        "UnEsm",
        "--until",
        "SmartInline",
        "--all",
    ])
    .expect("debug trace command should parse");

    match cli.command {
        Some(Command::Debug(DebugArgs {
            command: DebugCommand::Trace(args),
        })) => {
            assert_eq!(args.input, PathBuf::from("input.js"));
            assert_eq!(args.from.as_deref(), Some("UnEsm"));
            assert_eq!(args.until.as_deref(), Some("SmartInline"));
            assert!(args.all);
        }
        other => panic!("expected debug trace command, got {other:?}"),
    }
}

#[test]
fn parses_debug_normalize_command() {
    let cli = Cli::try_parse_from(["wakaru", "debug", "normalize", "input.js", "--rename"])
        .expect("debug normalize command should parse");

    match cli.command {
        Some(Command::Debug(DebugArgs {
            command: DebugCommand::Normalize(args),
        })) => {
            assert_eq!(args.input, Some(PathBuf::from("input.js")));
            assert!(args.rename);
            assert!(!args.format);
        }
        other => panic!("expected debug normalize command, got {other:?}"),
    }
}

#[test]
fn normalize_rename_then_format_canonicalizes_mangling() {
    // Mirrors run_normalize's pipeline: alpha-rename via core, then format.
    let opts = NormalizeOptions {
        rename_bindings: true,
        filename: "input.js".to_string(),
    };
    let fmt = |code: String| format_cli_output(code, "input.js", selected_formatter(true));
    let original = fmt(normalize("function load(app_id){return get(app_id)}", &opts).unwrap());
    let mangled = fmt(normalize("function l(e){return get(e)}", &opts).unwrap());
    assert_eq!(
        original, mangled,
        "mangled output should normalize identically"
    );
    assert!(original.contains("get"), "global preserved: {original}");
}

#[test]
fn parses_formatter_option() {
    let cli = Cli::try_parse_from(["wakaru", "input.js", "--formatter"])
        .expect("formatter option should parse");
    assert!(cli.formatter);
}

#[test]
fn parses_vue_sfc_option() {
    let cli = Cli::try_parse_from(["wakaru", "input.js", "--vue-sfc"]).expect("vue option parses");
    assert!(cli.vue_sfc);
}

#[test]
fn rejects_vue_sfc_with_raw_unpack() {
    let cli = Cli::try_parse_from([
        "wakaru",
        "bundle.js",
        "--unpack",
        "--raw",
        "--vue-sfc",
        "-o",
        "out",
    ])
    .expect("raw vue unpack args should parse before runtime validation");

    let err = run_default(cli).expect_err("raw vue output should be rejected");
    assert!(
        err.to_string()
            .contains("--vue-sfc cannot be combined with --raw"),
        "unexpected error: {err}"
    );
}

#[test]
fn vue_output_filename_replaces_known_extension() {
    assert_eq!(vue_output_filename("module-1.js"), "module-1.vue");
    assert_eq!(
        vue_output_filename("src/App.render.mjs"),
        "src/App.render.vue"
    );
    assert_eq!(vue_output_filename("module-plain"), "module-plain.vue");
}

#[test]
fn vue_js_output_filename_avoids_vue_artifact_collision() {
    assert_eq!(vue_js_output_filename("src/App.vue"), "src/App.vue.js");
    assert_eq!(vue_js_output_filename("src/App.js"), "src/App.js");
    assert_eq!(vue_js_output_filename("module-plain"), "module-plain");
}

#[test]
fn vue_sfc_writes_recovered_single_file_component() {
    let dir = temp_test_dir("vue-sfc-output");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("render.js");
    let output_path = dir.join("App.vue");
    fs::write(&input_path, vue_render_module_source()).expect("write vue render input");

    let cli = Cli::try_parse_from([
        "wakaru",
        input_path.to_str().expect("input path should be utf8"),
        "--vue-sfc",
        "-o",
        output_path.to_str().expect("output path should be utf8"),
    ])
    .expect("vue sfc cli should parse");
    run_default(cli).expect("vue sfc decompile should succeed");

    assert_eq!(
        fs::read_to_string(&output_path).expect("read vue sfc output"),
        "<script>\nexport default {\n    props: {\n        msg: String\n    }\n}\n</script>\n\n<template>\n  <div>{{ msg }}</div>\n</template>\n"
    );

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn vue_sfc_single_file_rejects_js_output_path_when_recovered() {
    let dir = temp_test_dir("vue-sfc-js-output");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("render.js");
    let output_path = dir.join("App.js");
    fs::write(&input_path, vue_render_module_source()).expect("write vue render input");

    let cli = Cli::try_parse_from([
        "wakaru",
        input_path.to_str().expect("input path should be utf8"),
        "--vue-sfc",
        "-o",
        output_path.to_str().expect("output path should be utf8"),
    ])
    .expect("vue sfc cli should parse");
    let err = run_default(cli).expect_err("recovered vue sfc should reject .js output");
    assert!(
        err.to_string()
            .contains("--vue-sfc recovered a Vue SFC but output path ends with .js"),
        "unexpected error: {err}"
    );

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn vue_sfc_single_file_does_not_write_source_map_for_recovered_sfc() {
    let dir = temp_test_dir("vue-sfc-output-map");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("render.js");
    let output_path = dir.join("App.vue");
    fs::write(&input_path, vue_render_module_source()).expect("write vue render input");

    let cli = Cli::try_parse_from([
        "wakaru",
        input_path.to_str().expect("input path should be utf8"),
        "--vue-sfc",
        "--emit-source-map",
        "-o",
        output_path.to_str().expect("output path should be utf8"),
    ])
    .expect("vue sfc cli should parse");
    run_default(cli).expect("vue sfc decompile should succeed");

    assert!(output_path.exists(), "recovered vue sfc should be written");
    assert!(
        !append_map_extension(&output_path).exists(),
        "recovered vue sfc must not get a stale JS source map"
    );

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn vue_sfc_relative_import_resolver_ignores_stdin_base() {
    assert_eq!(read_relative_import_source("<stdin>", "./main.js"), None);
}

#[test]
fn vue_sfc_relative_import_resolver_reads_extensionless_and_query_paths() {
    let dir = temp_test_dir("vue-sfc-relative-import-resolver");
    let components_dir = dir.join("components");
    fs::create_dir_all(&components_dir).expect("create temp dir");
    let input_path = dir.join("App.js");
    fs::write(&input_path, "export default {};").expect("write input");
    fs::write(components_dir.join("Child.vue"), "export default {};").expect("write component");
    fs::write(components_dir.join("Panel.js"), "export default {};").expect("write js module");
    fs::create_dir_all(components_dir.join("Dialog")).expect("create index dir");
    fs::write(
        components_dir.join("Dialog").join("index.vue"),
        "export default {};",
    )
    .expect("write index component");

    assert_eq!(
        read_relative_import_source(
            input_path.to_str().expect("input path should be utf8"),
            "./components/Child.vue?vue&type=script"
        ),
        Some("export default {};".to_string())
    );
    assert_eq!(
        read_relative_import_source(
            input_path.to_str().expect("input path should be utf8"),
            "./components/Child?vue&type=script"
        ),
        Some("export default {};".to_string())
    );
    assert_eq!(
        read_relative_import_source(
            input_path.to_str().expect("input path should be utf8"),
            "./components/Panel"
        ),
        Some("export default {};".to_string())
    );
    assert_eq!(
        read_relative_import_source(
            input_path.to_str().expect("input path should be utf8"),
            "./components/Dialog"
        ),
        Some("export default {};".to_string())
    );

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn vue_sfc_unpack_import_resolver_reads_root_relative_module_source() {
    let module_sources = HashMap::from([(
        "src/components/ChildPanel.vue".to_string(),
        "export default {};".to_string(),
    )]);

    assert_eq!(
        resolve_unpack_import_source(
            &module_sources,
            "src/App.vue",
            "./src/components/ChildPanel.vue"
        ),
        Some("export default {};".to_string())
    );
}

#[test]
fn vue_sfc_unpack_import_resolver_reads_module_relative_source() {
    let module_sources = HashMap::from([(
        "src/components/ChildPanel.vue".to_string(),
        "export default {};".to_string(),
    )]);

    assert_eq!(
        resolve_unpack_import_source(
            &module_sources,
            "src/App.vue",
            "./components/ChildPanel.vue"
        ),
        Some("export default {};".to_string())
    );
}

#[test]
fn vue_sfc_unpack_import_resolver_reads_extensionless_query_and_index_sources() {
    let module_sources = HashMap::from([
        (
            "src/components/ChildPanel.vue".to_string(),
            "export default {};".to_string(),
        ),
        (
            "src/components/Panel.js".to_string(),
            "export const panel = true;".to_string(),
        ),
        (
            "src/components/Dialog/index.vue".to_string(),
            "export const dialog = true;".to_string(),
        ),
    ]);

    assert_eq!(
        resolve_unpack_import_source(
            &module_sources,
            "src/App.vue",
            "./components/ChildPanel.vue?vue&type=script"
        ),
        Some("export default {};".to_string())
    );
    assert_eq!(
        resolve_unpack_import_source(
            &module_sources,
            "src/App.vue",
            "./components/ChildPanel?vue&type=script"
        ),
        Some("export default {};".to_string())
    );
    assert_eq!(
        resolve_unpack_import_source(&module_sources, "src/App.vue", "./components/Panel"),
        Some("export const panel = true;".to_string())
    );
    assert_eq!(
        resolve_unpack_import_source(&module_sources, "src/App.vue", "./components/Dialog"),
        Some("export const dialog = true;".to_string())
    );
}

#[test]
fn vue_sfc_unpack_import_resolver_reads_parent_relative_sources() {
    let module_sources = HashMap::from([(
        "src/components/ChildPanel.vue".to_string(),
        "export default {};".to_string(),
    )]);

    assert_eq!(
        resolve_unpack_import_source(
            &module_sources,
            "src/views/App.vue",
            "../components/ChildPanel"
        ),
        Some("export default {};".to_string())
    );
}

#[test]
fn vue_sfc_recovers_single_system_register_module() {
    let dir = temp_test_dir("vue-sfc-system-register");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("legacy.js");
    let output_path = dir.join("Recovered.vue");
    fs::write(
        &input_path,
        r#"
System.register(["./vendor-vue.js"], function (exports) {
  "use strict";
  var defineComponent, openBlock, createElementBlock;
  return {
    setters: [
      function (module) {
        defineComponent = module.d, openBlock = module.q, createElementBlock = module.X;
      }
    ],
    execute: function () {
      exports("_", defineComponent({
        __name: "LegacyGreeting",
        setup: function () {
          return function () {
            return openBlock(), createElementBlock("p", null, "Legacy");
          };
        }
      }));
    }
  };
});
"#,
    )
    .expect("write vue system register input");

    let cli = Cli::try_parse_from([
        "wakaru",
        input_path.to_str().expect("input path should be utf8"),
        "--vue-sfc",
        "-o",
        output_path.to_str().expect("output path should be utf8"),
    ])
    .expect("vue sfc cli should parse");
    run_default(cli).expect("vue sfc decompile should succeed");

    assert_eq!(
        fs::read_to_string(&output_path).expect("read vue sfc output"),
        "<template>\n  <p>Legacy</p>\n</template>\n"
    );

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn vue_sfc_resolves_relative_component_export_alias() {
    let dir = temp_test_dir("vue-sfc-relative-component");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("render.js");
    let shared_path = dir.join("main.js");
    let output_path = dir.join("Recovered.vue");
    fs::write(
        &input_path,
        r#"
import { q as ob, aa as cb, _ as rd } from "./vendor-vue.js";
import { B as B_1 } from "./main.js";
export function render(_ctx, _cache) {
  return ob(), cb(rd(B_1), { text: "Details" }, null, 8, ["text"]);
}
"#,
    )
    .expect("write vue render input");
    fs::write(
        &shared_path,
        r#"
import { defineComponent } from "vue";
const YP = defineComponent({
  name: "VTooltip",
  props: { text: String }
});
export { YP as B };
"#,
    )
    .expect("write shared component input");

    let cli = Cli::try_parse_from([
        "wakaru",
        input_path.to_str().expect("input path should be utf8"),
        "--vue-sfc",
        "-o",
        output_path.to_str().expect("output path should be utf8"),
    ])
    .expect("vue sfc cli should parse");
    run_default(cli).expect("vue sfc decompile should succeed");

    assert_eq!(
        fs::read_to_string(&output_path).expect("read vue sfc output"),
        "<script setup>\nimport { B as VTooltip } from \"./main.js\";\n</script>\n\n<template>\n  <VTooltip text=\"Details\" />\n</template>\n"
    );

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn vue_sfc_unpack_recovers_webpack_namespace_component() {
    let dir = temp_test_dir("vue-sfc-webpack");
    let out_dir = dir.join("out");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("bundle.js");
    fs::write(&input_path, webpack5_vue_sfc_bundle_source()).expect("write webpack vue bundle");

    let cli = Cli::try_parse_from([
        "wakaru",
        input_path.to_str().expect("input path should be utf8"),
        "--unpack",
        "--vue-sfc",
        "-o",
        out_dir.to_str().expect("output path should be utf8"),
    ])
    .expect("vue sfc unpack cli should parse");
    run_default(cli).expect("vue sfc webpack unpack should succeed");

    assert!(
        out_dir.join("src/App.vue.js").exists(),
        "decompiled JS should remain next to the recovered SFC"
    );
    assert_eq!(
        fs::read_to_string(out_dir.join("src/App.vue")).expect("read recovered vue sfc"),
        "<script>\nexport default {\n    name: \"WebpackPanel\",\n    props: {\n        message: String\n    }\n}\n</script>\n\n<script setup>\nimport ChildPanel from \"./src/components/ChildPanel.vue\";\n</script>\n\n<template>\n  <section class=\"notice\">\n    <ChildPanel :label=\"message\" />\n    <span>{{ message }}</span>\n  </section>\n</template>\n"
    );

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn vue_sfc_unpack_writes_source_maps_only_for_js_artifacts() {
    let dir = temp_test_dir("vue-sfc-webpack-map");
    let out_dir = dir.join("out");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("bundle.js");
    fs::write(&input_path, webpack5_vue_sfc_bundle_source()).expect("write webpack vue bundle");

    let cli = Cli::try_parse_from([
        "wakaru",
        input_path.to_str().expect("input path should be utf8"),
        "--unpack",
        "--vue-sfc",
        "--emit-source-map",
        "-o",
        out_dir.to_str().expect("output path should be utf8"),
    ])
    .expect("vue sfc unpack cli should parse");
    run_default(cli).expect("vue sfc webpack unpack should succeed");

    assert!(
        out_dir.join("src/App.vue").exists(),
        "recovered vue sfc should be written"
    );
    assert!(
        out_dir.join("src/App.vue.js").exists(),
        "decompiled JS should be written"
    );
    assert!(
        out_dir.join("src/App.vue.js.map").exists(),
        "decompiled JS should keep its source map"
    );
    assert!(
        !out_dir.join("src/App.vue.map").exists(),
        "recovered vue sfc must not get a stale JS source map"
    );

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn parses_json_flag() {
    let cli =
        Cli::try_parse_from(["wakaru", "input.js", "--json"]).expect("json flag should parse");
    assert!(cli.json);
}

#[test]
fn parses_json_with_unpack() {
    let cli = Cli::try_parse_from(["wakaru", "bundle.js", "--unpack", "--json", "-o", "out"])
        .expect("json with unpack should parse");
    assert!(cli.json);
    assert!(cli.unpack.is_some());
}

#[test]
fn json_modules_describe_vue_sfc_artifact_roles() {
    let modules = vec![
        json_module_for_artifact(&CliOutputArtifact {
            filename: "src/plain.js".to_string(),
            code: "export {};".to_string(),
            kind: JsonModuleKind::JavaScript,
            status: JsonModuleStatus::Decompiled,
            source_filename: None,
            source_map_filename: Some("src/plain.js".to_string()),
        }),
        json_module_for_artifact(&CliOutputArtifact {
            filename: "src/App.vue.js".to_string(),
            code: "export {};".to_string(),
            kind: JsonModuleKind::JavaScript,
            status: JsonModuleStatus::VueSfcSourceJs,
            source_filename: Some("src/App.vue".to_string()),
            source_map_filename: Some("src/App.vue".to_string()),
        }),
        json_module_for_artifact(&CliOutputArtifact {
            filename: "src/App.vue".to_string(),
            code: "<template />".to_string(),
            kind: JsonModuleKind::VueSfc,
            status: JsonModuleStatus::RecoveredVueSfc,
            source_filename: Some("src/App.vue".to_string()),
            source_map_filename: None,
        }),
        json_module_for_artifact(&CliOutputArtifact {
            filename: "src/Broken.vue.js".to_string(),
            code: "export {};".to_string(),
            kind: JsonModuleKind::JavaScript,
            status: JsonModuleStatus::VueSfcFallbackJs,
            source_filename: Some("src/Broken.vue".to_string()),
            source_map_filename: Some("src/Broken.vue".to_string()),
        }),
    ];

    assert_eq!(
        serde_json::to_value(modules).expect("serialize modules"),
        serde_json::json!([
            {
                "filename": "src/plain.js",
                "kind": "javascript",
                "status": "decompiled"
            },
            {
                "filename": "src/App.vue.js",
                "kind": "javascript",
                "status": "vue_sfc_source_js",
                "source_filename": "src/App.vue"
            },
            {
                "filename": "src/App.vue",
                "kind": "vue_sfc",
                "status": "recovered_vue_sfc",
                "source_filename": "src/App.vue"
            },
            {
                "filename": "src/Broken.vue.js",
                "kind": "javascript",
                "status": "vue_sfc_fallback_js",
                "source_filename": "src/Broken.vue"
            }
        ])
    );
}

#[test]
fn format_elapsed_uses_seconds_for_long_durations() {
    let d = Duration::from_millis(1234);
    assert_eq!(format_elapsed(d), "1.23s");
}

#[test]
fn format_elapsed_uses_millis_for_short_durations() {
    let d = Duration::from_millis(456);
    assert_eq!(format_elapsed(d), "456ms");
}

#[test]
fn format_elapsed_zero() {
    let d = Duration::from_millis(0);
    assert_eq!(format_elapsed(d), "0ms");
}

#[test]
fn parses_formatter_with_raw_unpack() {
    let cli = Cli::try_parse_from([
        "wakaru",
        "bundle.js",
        "--unpack",
        "--raw",
        "--formatter",
        "-o",
        "out",
    ])
    .expect("formatter with raw should parse");

    assert!(cli.raw);
    assert!(cli.formatter);
}

#[test]
fn parses_multiple_unpack_inputs() {
    let cli = Cli::try_parse_from([
        "wakaru",
        "--unpack",
        "-o",
        "out",
        "bundle.js",
        "src_greet_js.bundle.js",
    ])
    .expect("multi-file unpack should parse");

    assert!(cli.unpack.is_some());
    assert_eq!(
        cli.inputs,
        vec![
            PathBuf::from("bundle.js"),
            PathBuf::from("src_greet_js.bundle.js")
        ]
    );
}

#[test]
fn parses_profile_flag() {
    let cli = Cli::try_parse_from(["wakaru", "input.js", "--profile", "profile.json"])
        .expect("--profile should parse");
    assert_eq!(cli.profile, Some(PathBuf::from("profile.json")));
    assert!(!cli.profile_rules);
}

#[test]
fn parses_profile_rules_flag() {
    let cli = Cli::try_parse_from([
        "wakaru",
        "input.js",
        "--profile",
        "profile.json",
        "--profile-rules",
    ])
    .expect("--profile-rules should parse with --profile");
    assert!(cli.profile_rules);
}

#[test]
fn rejects_profile_rules_without_profile() {
    assert!(Cli::try_parse_from(["wakaru", "input.js", "--profile-rules"]).is_err());
}

#[test]
fn parses_source_map_aliases() {
    let cli = Cli::try_parse_from(["wakaru", "input.js", "--source-map", "input.js.map"])
        .expect("--source-map should parse");
    assert_eq!(cli.sourcemap, Some(PathBuf::from("input.js.map")));

    let cli = Cli::try_parse_from(["wakaru", "input.js", "--sourcemap", "input.js.map"])
        .expect("--sourcemap alias should parse");
    assert_eq!(cli.sourcemap, Some(PathBuf::from("input.js.map")));
}

#[test]
fn decompile_rejects_directory_input() {
    let dir = temp_test_dir("decompile-dir");
    fs::create_dir_all(&dir).expect("create temp dir");

    let cli = Cli::try_parse_from(["wakaru", dir.to_str().expect("temp path should be utf8")])
        .expect("directory input should parse");
    let err = run_default(cli).expect_err("decompile should reject directory input");
    assert!(
        err.to_string()
            .contains("cannot decompile a directory. Pass a JavaScript file or use --unpack"),
        "unexpected error: {err}"
    );

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn unpack_directory_inputs_are_recursive_detected_js_files_only() {
    let dir = temp_test_dir("unpack-dir");
    let nested = dir.join("nested");
    let hidden = dir.join(".hidden");
    let node_modules = dir.join("node_modules");
    fs::create_dir_all(&nested).expect("create nested dir");
    fs::create_dir_all(&hidden).expect("create hidden dir");
    fs::create_dir_all(&node_modules).expect("create node_modules dir");

    fs::write(dir.join("plain.js"), "const value = 1;").expect("write plain file");
    fs::write(dir.join("runtime-like.js"), runtime_like_plain_source())
        .expect("write runtime-like plain file");
    fs::write(nested.join("chunk.js"), webpack5_chunk_source()).expect("write chunk");
    fs::write(dir.join("runtime.js"), webpack5_runtime_entry_source())
        .expect("write runtime entry");
    fs::write(hidden.join("hidden.js"), webpack5_chunk_source()).expect("write hidden chunk");
    fs::write(node_modules.join("vendor.js"), webpack5_chunk_source())
        .expect("write node_modules chunk");
    fs::write(dir.join("chunk.js.map"), webpack5_chunk_source()).expect("write sourcemap");

    let input_set =
        read_unpack_inputs(std::slice::from_ref(&dir), false).expect("read directory inputs");
    assert_eq!(
        input_set.scan_stats,
        Some(DirectoryScanStats {
            scanned: 4,
            detected: 2,
            skipped: 2,
        })
    );
    assert_eq!(
        input_set.inputs.len(),
        2,
        "expected visible chunk and runtime entry"
    );
    assert!(
        input_set
            .inputs
            .iter()
            .any(|input| input.filename.ends_with("nested\\chunk.js")
                || input.filename.ends_with("nested/chunk.js")),
        "missing detected chunk input: {:?}",
        input_set.inputs
    );
    assert!(
        input_set
            .inputs
            .iter()
            .any(|input| input.filename.ends_with("runtime.js")),
        "missing detected runtime input: {:?}",
        input_set.inputs
    );

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn unpack_directory_without_detected_files_errors() {
    let dir = temp_test_dir("unpack-dir-empty");
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(dir.join("plain.js"), "const value = 1;").expect("write plain file");

    let err = read_unpack_inputs(std::slice::from_ref(&dir), false)
        .expect_err("directory with no detected bundles should error");
    assert!(
        err.to_string()
            .contains("no bundle or chunk files detected in directory input"),
        "unexpected error: {err}"
    );

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn output_file_requires_force_to_overwrite() {
    let dir = temp_test_dir("output-file");
    fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("out.js");
    fs::write(&path, "old").expect("write temp file");

    assert!(ensure_output_file(&path, false).is_err());
    assert!(ensure_output_file(&path, true).is_ok());

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn output_dir_requires_force_when_non_empty() {
    let dir = temp_test_dir("output-dir");
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(dir.join("entry.js"), "old").expect("write temp file");

    assert!(ensure_output_dir(&dir, false).is_err());
    assert!(ensure_output_dir(&dir, true).expect("force should allow non-empty dir"));

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn unpack_cli_does_not_write_overlapping_dot_payload_outside_output_dir() {
    let dir = temp_test_dir("unpack-cli-overlap");
    let out_dir = dir.join("out");
    let bundle_path = dir.join("bundle.js");
    let outside_target = dir.join("node_modules/@wakaru/cli/bin/wakaru");
    fs::create_dir_all(outside_target.parent().expect("outside target parent"))
        .expect("create outside target parent");
    fs::write(&outside_target, "original").expect("write outside marker");
    fs::write(&bundle_path, overlapping_dot_webpack5_bundle()).expect("write bundle");

    let cli = Cli::try_parse_from([
        "wakaru",
        bundle_path.to_str().expect("bundle path should be utf8"),
        "--unpack",
        "-o",
        out_dir.to_str().expect("output path should be utf8"),
    ])
    .expect("cli should parse");
    run_default(cli).expect("unpack should succeed");

    assert_eq!(
        fs::read_to_string(&outside_target).expect("read outside marker"),
        "original",
        "outside marker must not be overwritten"
    );
    assert!(
        out_dir
            .join("..../node_modules/@wakaru/cli/bin/wakaru")
            .exists(),
        "payload should be written under the output directory"
    );

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn output_dir_reports_when_existing_writes_need_checks() {
    let empty_dir = temp_test_dir("output-dir-empty");
    fs::create_dir_all(&empty_dir).expect("create temp dir");
    assert!(
        !ensure_output_dir(&empty_dir, false).expect("empty dir should be accepted"),
        "empty directories can write directly without checking existing files"
    );
    fs::remove_dir_all(&empty_dir).expect("remove empty temp dir");

    let new_dir = temp_test_dir("output-dir-new");
    assert!(
        !ensure_output_dir(&new_dir, false).expect("new dir should be created"),
        "new directories can write directly without checking existing files"
    );
    fs::remove_dir_all(&new_dir).expect("remove new temp dir");

    let non_empty_dir = temp_test_dir("output-dir-non-empty");
    fs::create_dir_all(&non_empty_dir).expect("create temp dir");
    fs::write(non_empty_dir.join("entry.js"), "old").expect("write temp file");
    assert!(
        ensure_output_dir(&non_empty_dir, true).expect("force should allow non-empty dir"),
        "non-empty forced directories should preserve write-if-changed checks"
    );
    fs::remove_dir_all(&non_empty_dir).expect("remove non-empty temp dir");
}

fn temp_test_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("wakaru-cli-test-{name}-{nanos}"))
}

fn vue_render_module_source() -> &'static str {
    r#"
import { toDisplayString as _toDisplayString, openBlock as _openBlock, createElementBlock as _createElementBlock } from "vue";
const __sfc__ = { props: { msg: String } };
export function render(_ctx, _cache) {
  return (_openBlock(), _createElementBlock("div", null, _toDisplayString(_ctx.msg), 1));
}
__sfc__.render = render;
export default __sfc__;
"#
}

fn webpack5_chunk_source() -> &'static str {
    r#"
(self.webpackChunk = self.webpackChunk || []).push([
  [1],
  {
    100: function(module, exports, require) {
      "use strict";
      require.r(exports);
      exports.default = 1;
    }
  }
]);
"#
}

fn webpack5_runtime_entry_source() -> &'static str {
    r#"
(() => {
  var modules = {};
  function require(id) { return {}; }
  require.m = modules;
  require.f = {};
  require.e = function(id) { return Promise.resolve(id); };
  require.u = function(id) { return id + ".bundle.js"; };
  require.t = function(value) { return value; };
  require.e(529).then(require.t.bind(require, 529, 19));
})();
"#
}

fn webpack5_vue_sfc_bundle_source() -> &'static str {
    r#"
(() => {
  var __webpack_modules__ = ({
    "./node_modules/vue/index.js": ((__unused_webpack_module, __webpack_exports__, __webpack_require__) => {
      __webpack_require__.r(__webpack_exports__);
      __webpack_require__.d(__webpack_exports__, {
        createElementBlock: () => createElementBlock,
        createVNode: () => createVNode,
        defineComponent: () => defineComponent,
        openBlock: () => openBlock,
        toDisplayString: () => toDisplayString
      });
      function createElementBlock() {}
      function createVNode() {}
      function defineComponent(options) { return options; }
      function openBlock() {}
      function toDisplayString(value) { return String(value); }
    }),
    "./src/components/ChildPanel.vue": ((__unused_webpack_module, __webpack_exports__, __webpack_require__) => {
      __webpack_require__.r(__webpack_exports__);
      __webpack_require__.d(__webpack_exports__, { default: () => __WEBPACK_DEFAULT_EXPORT__ });
      var vue__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__("./node_modules/vue/index.js");
      const __WEBPACK_DEFAULT_EXPORT__ = (0, vue__WEBPACK_IMPORTED_MODULE_0__.defineComponent)({
        name: "ChildPanel",
        props: { label: String }
      });
    }),
    "./src/App.vue": ((__unused_webpack_module, __webpack_exports__, __webpack_require__) => {
      __webpack_require__.r(__webpack_exports__);
      __webpack_require__.d(__webpack_exports__, { default: () => __WEBPACK_DEFAULT_EXPORT__ });
      var vue__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__("./node_modules/vue/index.js");
      var _components_ChildPanel_vue__WEBPACK_IMPORTED_MODULE_1__ = __webpack_require__("./src/components/ChildPanel.vue");
      const _hoisted_1 = { class: "notice" };
      function render(_ctx, _cache) {
        return (0, vue__WEBPACK_IMPORTED_MODULE_0__.openBlock)(), (0, vue__WEBPACK_IMPORTED_MODULE_0__.createElementBlock)("section", _hoisted_1, [
          (0, vue__WEBPACK_IMPORTED_MODULE_0__.createVNode)(_components_ChildPanel_vue__WEBPACK_IMPORTED_MODULE_1__["default"], { label: _ctx.message }, null, 8, ["label"]),
          (0, vue__WEBPACK_IMPORTED_MODULE_0__.createElementBlock)("span", null, (0, vue__WEBPACK_IMPORTED_MODULE_0__.toDisplayString)(_ctx.message), 1)
        ]);
      }
      const __WEBPACK_DEFAULT_EXPORT__ = (0, vue__WEBPACK_IMPORTED_MODULE_0__.defineComponent)({
        name: "WebpackPanel",
        props: { message: String },
        render
      });
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var cachedModule = __webpack_module_cache__[moduleId];
    if (cachedModule !== undefined) return cachedModule.exports;
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__.d = (exports, definition) => {
    for (var key in definition) {
      if (__webpack_require__.o(definition, key) && !__webpack_require__.o(exports, key)) {
        Object.defineProperty(exports, key, { enumerable: true, get: definition[key] });
      }
    }
  };
  __webpack_require__.o = (obj, prop) => Object.prototype.hasOwnProperty.call(obj, prop);
  __webpack_require__.r = (exports) => {
    if (typeof Symbol !== "undefined" && Symbol.toStringTag) {
      Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
    }
    Object.defineProperty(exports, "__esModule", { value: true });
  };
  __webpack_require__("./src/App.vue");
})();
"#
}

fn runtime_like_plain_source() -> &'static str {
    r#"
(() => {
  const api = {};
  api.e = 1;
  api.u = 2;
  api.t = 3;
  api.m = 4;
})();
"#
}

fn overlapping_dot_webpack5_bundle() -> &'static str {
    r#"
(() => {
  var __webpack_modules__ = ({
    "....//node_modules/@wakaru/cli/bin/wakaru": ((module) => {
      module.exports = "pwned";
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  console.log(__webpack_require__("....//node_modules/@wakaru/cli/bin/wakaru"));
})();
"#
}

#[test]
fn renders_provenance_json_with_final_names_and_default_input() {
    let provenance = vec![
        wakaru_core::ModuleProvenance {
            filename: "b.js".to_string(),
            input: String::new(),
            ranges: vec![(10, 20), (30, 40)],
        },
        wakaru_core::ModuleProvenance {
            filename: "a \"quoted\".js".to_string(),
            input: "chunk-1.js".to_string(),
            ranges: vec![(0, 5)],
        },
        wakaru_core::ModuleProvenance {
            filename: "module-1/chunk_a.js".to_string(),
            input: String::new(),
            ranges: vec![(50, 60)],
        },
    ];
    let mut final_names = HashMap::new();
    // CLI-side dedup renamed b.js on disk.
    final_names.insert("b.js", "b_2.js".to_string());

    let json = render_provenance_json(
        &provenance,
        &final_names,
        "bundle.js",
        &[wakaru_core::BundleFormat::Webpack5],
    );

    assert!(
        json.contains(r#""format": "webpack5""#),
        "format metadata missing:\n{json}"
    );
    assert!(
        json.contains(r#""strategy": "mixed""#),
        "strategy metadata missing:\n{json}"
    );
    assert!(
        json.contains(r#""b_2.js": {"input": "bundle.js", "ranges": [[10,20],[30,40]], "extraction": "structural"}"#),
        "renamed module with default input missing:\n{json}"
    );
    assert!(
        json.contains(r#""a \"quoted\".js": {"input": "chunk-1.js", "ranges": [[0,5]], "extraction": "structural"}"#),
        "escaped filename with explicit input missing:\n{json}"
    );
    assert!(
        json.contains(r#""module-1/chunk_a.js": {"input": "bundle.js", "ranges": [[50,60]], "extraction": "heuristic"}"#),
        "nested heuristic module metadata missing:\n{json}"
    );
    // Must be alphabetically sorted and valid JSON shape.
    assert!(json.find("a \\\"quoted\\\"").unwrap() < json.find("b_2.js").unwrap());
    assert!(json.starts_with(
        "{\n  \"format\": \"webpack5\",\n  \"strategy\": \"mixed\",\n  \"modules\": {\n"
    ));
    assert!(json.ends_with("  }\n}\n"));
}
