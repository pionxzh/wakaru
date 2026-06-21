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
