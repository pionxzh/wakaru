use std::collections::HashSet;

use super::*;
use crate::test_tracing::record_spans;
use crate::unpacker::UnpackedModule;

#[test]
fn prepared_input_classifies_unrecoverable_parse_errors() {
    let error = prepare_unpack_input(
        "broken.js".to_string(),
        "function (".to_string(),
        false,
        true,
    )
    .err()
    .expect("invalid input should fail preparation");

    assert_eq!(error.kind(), DriverErrorKind::Parse);
}

#[test]
fn prepared_input_coalesces_repeated_recoverable_parse_signatures() {
    let input = prepare_unpack_input(
        "classic-script.js".to_string(),
        "with (first) { consume(first); }\nwith (second) { consume(second); }".to_string(),
        false,
        true,
    )
    .expect("classic script should recover during preparation");
    let output = unpack_prepared_inputs(vec![input], DecompileOptions::default(), false, false)
        .expect("prepared classic script should decompile");
    let warnings = output
        .warnings
        .iter()
        .filter(|warning| warning.kind == UnpackWarningKind::InputParseRecovered)
        .collect::<Vec<_>>();

    assert_eq!(warnings.len(), 1, "warnings should coalesce: {warnings:#?}");
    assert!(warnings[0].message.contains("WithInStrict"));
    assert!(warnings[0].message.contains("2 occurrences"));
    assert!(warnings[0].message.contains("classic-script.js:1:1"));
}

#[test]
fn prepared_plain_input_reuses_detection_ast_in_phase1() {
    let (output, spans) = record_spans(|| {
        let input = prepare_unpack_input(
            "plain.js".to_string(),
            "const answer = 40 + 2;".to_string(),
            false,
            true,
        )
        .expect("plain input should prepare");
        assert_eq!(input.detection(), PreparedInputDetection::Plain);

        unpack_prepared_inputs(vec![input], DecompileOptions::default(), false, false)
            .expect("prepared plain input should decompile")
    });

    assert_eq!(output.modules.len(), 1);
    assert!(output.modules[0].code.contains("answer"));
    assert_eq!(
        spans.iter().filter(|name| *name == "parse_bundle").count(),
        1,
        "detection should parse the input exactly once: {spans:?}"
    );
    for skipped in ["phase1: parse", "phase1: resolver"] {
        assert!(
            !spans.iter().any(|name| name == skipped),
            "unexpected prepared-input round trip {skipped:?} in {spans:?}"
        );
    }
    assert!(spans.iter().any(|name| name == "prepare_plain: resolver"));
}

#[test]
fn prepared_inputs_reject_duplicate_physical_identity_names() {
    let inputs = ["const first = 1;", "const second = 2;"]
        .into_iter()
        .map(|source| {
            prepare_unpack_input("same.js".to_string(), source.to_string(), false, true)
                .expect("plain input should prepare")
        })
        .collect();

    let error = unpack_prepared_inputs(inputs, DecompileOptions::default(), false, false)
        .expect_err("duplicate physical identities must fail before emission");
    assert!(
        error.to_string().contains("ambiguous public module path"),
        "unexpected error: {error}"
    );
}

#[test]
fn prepared_output_uses_typed_input_indices_for_distinct_paths() {
    let inputs = [
        ("first/same.js", "const first = 1;"),
        ("second/same.js", "const second = 2;"),
    ]
    .into_iter()
    .map(|(filename, source)| {
        prepare_unpack_input(filename.to_string(), source.to_string(), false, true)
            .expect("plain input should prepare")
    })
    .collect();

    let output = unpack_prepared_inputs(inputs, DecompileOptions::default(), false, false)
        .expect("distinct prepared inputs should decompile");

    assert_eq!(output.modules.len(), 2);
    assert_eq!(output.modules[0].filename, "first/same.js");
    assert_eq!(output.modules[1].filename, "second/same.js");
    assert_eq!(
        output.modules[0]
            .provenance
            .input
            .map(PreparedInputId::index),
        Some(0)
    );
    assert_eq!(
        output.modules[1]
            .provenance
            .input
            .map(PreparedInputId::index),
        Some(1)
    );
}

#[test]
fn legacy_plain_unpack_uses_prepared_intake_once() {
    let (output, spans) = record_spans(|| {
        unpack(
            "const answer = 40 + 2;",
            DecompileOptions {
                filename: "src/input.js".to_string(),
                ..Default::default()
            },
        )
        .expect("legacy plain input should decompile")
    });

    assert_eq!(output.modules[0].0, "module.js");
    assert_eq!(output.provenance[0].filename, "module.js");
    assert!(output.provenance[0].input.is_empty());
    assert_eq!(
        spans.iter().filter(|name| *name == "parse_bundle").count(),
        1,
        "legacy intake should delegate to preparation exactly once: {spans:?}"
    );
    assert!(
        !spans.iter().any(|name| name == "phase1: parse"),
        "legacy plain intake should reuse its prepared AST: {spans:?}"
    );
}

#[test]
fn unprocessed_plain_input_skips_resolver_preparation() {
    let (detection, spans) = record_spans(|| {
        prepare_unpack_input(
            "plain.js".to_string(),
            "const value = 1;".to_string(),
            false,
            false,
        )
        .expect("plain input should detect")
        .detection()
    });
    assert_eq!(detection, PreparedInputDetection::Plain);
    assert!(
        !spans.iter().any(|name| name == "prepare_plain: resolver"),
        "unprocessed plain input should only be detected: {spans:?}"
    );
}

#[test]
fn prepared_raw_scope_split_keeps_runnable_normalization() {
    let input = PreparedUnpackInput {
        filename: "bundle.js".to_string(),
        source: None,
        detection: PreparedInputDetection::ScopeHoisted,
        detected: None,
        scope_hoisted: Some(UnpackResult {
            modules: vec![UnpackedModule {
                id: "entry".to_string(),
                is_entry: true,
                filename: "entry.js".to_string(),
                source_ranges: vec![(0, 18)],
                inspection_context_ranges: Vec::new(),
                source_input: String::new(),
                generated_source_map: Vec::new(),
                code: "if (ready) run();".to_string(),
            }],
            report_import_cycle_warnings: false,
            format: BundleFormat::ScopeHoisted,
        }),
        plain_prepared: None,
        public_path_candidate: true,
    };

    let output = unpack_prepared_inputs(vec![input], DecompileOptions::default(), true, false)
        .expect("raw scope-hoisted module should normalize");
    assert_eq!(output.modules.len(), 1);
    assert!(
        output.modules[0].code.contains("if (ready) {"),
        "raw split should retain runnable statement normalization: {}",
        output.modules[0].code
    );
}

#[test]
fn unprovable_public_boundary_falls_back_to_one_processed_module() {
    let source = "export const intact = 1;";
    let uncertain = PreparedUnpackInput {
        filename: "uncertain.js".to_string(),
        source: Some(source.to_string()),
        detection: PreparedInputDetection::ScopeHoisted,
        detected: None,
        scope_hoisted: Some(UnpackResult {
            modules: vec![
                UnpackedModule {
                    filename: "chunk_a.js".to_string(),
                    code: "export const a = 1;".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    filename: "chunk_b.js".to_string(),
                    code: "export const b = 2;".to_string(),
                    ..Default::default()
                },
            ],
            report_import_cycle_warnings: false,
            format: BundleFormat::ScopeHoisted,
        }),
        plain_prepared: None,
        public_path_candidate: true,
    };
    let sibling = prepare_unpack_input(
        "consumer.js".to_string(),
        "import { intact } from './uncertain.js'; console.log(intact);".to_string(),
        false,
        true,
    )
    .expect("sibling should prepare");

    let output = unpack_prepared_inputs(
        vec![uncertain, sibling],
        DecompileOptions::default(),
        false,
        false,
    )
    .expect("uncertain boundary should fall back safely");

    let input_modules = output
        .modules
        .iter()
        .filter(|module| module.provenance.input == Some(PreparedInputId::from_index(0)))
        .collect::<Vec<_>>();
    assert_eq!(input_modules.len(), 1);
    assert_eq!(input_modules[0].filename, "uncertain.js");
    assert!(input_modules[0].code.contains("intact"));
}

#[test]
fn prepared_webpack_input_does_not_reparse_for_chunk_metadata() {
    let source = r#"
(self.webpackChunkapp = self.webpackChunkapp || []).push([[1], {
    1: (module) => { module.exports = 1; }
}]);
"#;
    let (output, spans) = record_spans(|| {
        let input = prepare_unpack_input("chunk.js".to_string(), source.to_string(), false, true)
            .expect("webpack input should prepare");
        assert!(matches!(
            input.detection(),
            PreparedInputDetection::Bundle(BundleFormat::Webpack5)
        ));
        unpack_prepared_inputs(vec![input], DecompileOptions::default(), false, false)
            .expect("prepared webpack input should decompile")
    });

    assert_eq!(output.modules.len(), 1);
    assert_eq!(
        spans.iter().filter(|name| *name == "parse_bundle").count(),
        1,
        "chunk metadata must come from the detection AST: {spans:?}"
    );
    for skipped in [
        "unpacker: prepared emit",
        "phase1: parse",
        "phase1: resolver",
    ] {
        assert!(
            !spans.iter().any(|name| name == skipped),
            "unexpected prepared-input round trip {skipped:?} in {spans:?}"
        );
    }
}

#[test]
fn webpack5_normal_unpack_consumes_prepared_ast_without_round_trip() {
    let source = r#"
(self.webpackChunkapp = self.webpackChunkapp || []).push([[1], {
    1: (module, exports, require) => {
        module.exports = { value: 1 };
    }
}]);
"#;

    let (output, spans) = record_spans(|| {
        unpack(
            source,
            DecompileOptions {
                filename: "chunk.js".to_string(),
                heuristic_split: false,
                ..Default::default()
            },
        )
        .expect("webpack chunk should unpack")
    });

    assert_eq!(output.modules.len(), 1);
    assert!(output.modules[0].1.contains("value: 1"));
    for expected in [
        "webpack5: prepare_module",
        "phase1: rules",
        "phase2: rules",
        "phase2: emit",
    ] {
        assert!(
            spans.iter().any(|name| name == expected),
            "missing {expected:?} in {spans:?}"
        );
    }
    for skipped in [
        "unpacker: prepared emit",
        "phase1: parse",
        "phase1: resolver",
    ] {
        assert!(
            !spans.iter().any(|name| name == skipped),
            "unexpected round-trip span {skipped:?} in {spans:?}"
        );
    }
}

#[test]
fn browserify_family_normal_unpack_consumes_prepared_ast_without_round_trip() {
    let source = r#"
window.__require = function(modules, cache, entries) { return function() {}; }({
    Entry: [function(require, module, exports) {
        cc._RF.push(module, "entryFixtureUuid", "Entry");
        exports.value = 1;
        cc._RF.pop();
    }, {}]
}, {}, ["Entry"]);
"#;

    let (output, spans) = record_spans(|| {
        unpack(
            source,
            DecompileOptions {
                filename: "project.js".to_string(),
                heuristic_split: false,
                ..Default::default()
            },
        )
        .expect("Cocos Creator bundle should unpack")
    });

    assert_eq!(output.modules.len(), 1);
    assert!(output.modules[0].1.contains("value = 1"));
    for expected in ["phase1: rules", "phase2: rules", "phase2: emit"] {
        assert!(
            spans.iter().any(|name| name == expected),
            "missing {expected:?} in {spans:?}"
        );
    }
    for skipped in [
        "unpacker: prepared emit",
        "phase1: parse",
        "phase1: resolver",
    ] {
        assert!(
            !spans.iter().any(|name| name == skipped),
            "unexpected round-trip span {skipped:?} in {spans:?}"
        );
    }
}

#[test]
fn webpack5_source_map_mode_materializes_before_phase1() {
    let source = r#"
(self.webpackChunkapp = self.webpackChunkapp || []).push([[1], {
    1: (module) => {
        module.exports = 1;
    }
}]);
"#;

    let (output, spans) = record_spans(|| {
        unpack(
            source,
            DecompileOptions {
                filename: "chunk.js".to_string(),
                heuristic_split: false,
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("webpack chunk should unpack with an output source map")
    });

    assert_eq!(output.modules.len(), 1);
    assert_eq!(output.source_maps.len(), 1);
    for expected in [
        "unpacker: prepared emit",
        "phase1: parse",
        "phase1: resolver",
    ] {
        assert!(
            spans.iter().any(|name| name == expected),
            "missing materialized-path span {expected:?} in {spans:?}"
        );
    }
}

#[test]
fn nested_scope_split_gate_requires_heuristic_split_and_aggressive() {
    assert!(!nested_scope_split_enabled(&DecompileOptions {
        heuristic_split: false,
        level: RewriteLevel::Aggressive,
        ..Default::default()
    }));
    assert!(!nested_scope_split_enabled(&DecompileOptions {
        heuristic_split: true,
        level: RewriteLevel::Standard,
        ..Default::default()
    }));
    assert!(!nested_scope_split_enabled(&DecompileOptions {
        heuristic_split: true,
        level: RewriteLevel::Minimal,
        ..Default::default()
    }));
    assert!(nested_scope_split_enabled(&DecompileOptions {
        heuristic_split: true,
        level: RewriteLevel::Aggressive,
        ..Default::default()
    }));
}

#[test]
fn scan_local_import_dependencies_reads_static_imports() {
    let module_names = ["a.js".to_string(), "nested/b.js".to_string()]
        .into_iter()
        .collect();
    let deps = scan_local_import_dependencies(
        "nested/current.js",
        r#"
import { a } from "../a.js";
import {
  b
} from "./b.js";
import fs from "fs";
const value = import("./dynamic.js");
"#,
        &module_names,
    )
    .expect("static imports should scan without parsing");

    assert_eq!(deps, vec!["a.js".to_string(), "nested/b.js".to_string()]);
}

#[test]
fn scan_local_import_dependencies_ignores_import_like_body_code() {
    let module_names = ["dynamic.js".to_string()].into_iter().collect();
    let deps = scan_local_import_dependencies(
        "entry.js",
        r#"
const value = "import './dynamic.js'";
import("./dynamic.js");
"#,
        &module_names,
    )
    .expect("non-import prefix should still be a valid fast scan");

    assert!(deps.is_empty());
}

#[test]
fn scan_local_import_dependencies_ignores_nested_import_like_lines() {
    let module_names = ["nested.js".to_string()].into_iter().collect();
    let deps = scan_local_import_dependencies(
        "entry.js",
        r#"
function load() {
  import { nested } from "./nested.js";
}
"#,
        &module_names,
    )
    .expect("nested import-like code should still scan without parsing");

    assert!(deps.is_empty());
}

#[test]
fn unpack_raw_preserves_unparseable_extracted_modules() {
    let result = unpack_raw(
        "const = ;",
        &DecompileOptions {
            heuristic_split: false,
            ..Default::default()
        },
    );

    assert!(result.is_err(), "invalid top-level input should still fail");

    let modules = vec![UnpackedModule {
        id: "1".to_string(),
        is_entry: false,
        code: "const = ;".to_string(),
        filename: "module-1.js".to_string(),
        ..Default::default()
    }];
    let output = unpack_multi_module(modules, DecompileOptions::default())
        .expect("unparseable extracted modules should be preserved as raw code");
    assert_eq!(output.modules.len(), 1);
    assert_eq!(output.modules[0].filename, "module-1.js");
    assert_eq!(output.modules[0].code, "const = ;");
    assert!(
        !output.warnings.is_empty(),
        "should warn about unparseable module"
    );
    let warning_kinds = output
        .warnings
        .iter()
        .map(|warning| {
            assert_eq!(warning.filename, "module-1.js");
            warning.kind
        })
        .collect::<Vec<_>>();
    assert_eq!(
        warning_kinds,
        vec![
            UnpackWarningKind::FactCollectionParseFailed,
            UnpackWarningKind::DecompileFailed
        ]
    );
}

#[test]
fn detector_raw_large_scope_split_skips_runnable_cleanup_merge() {
    let mut source = String::from(
        r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { a: () => a });
function a() { return b(); }
var ns_b = {};
__export(ns_b, { b: () => b });
function b() { return a(); }
"#,
    );
    for index in 0..1000 {
        source.push_str(&format!(
                "var ns_{index} = {{}};\n__export(ns_{index}, {{ v{index}: () => v{index} }});\nvar v{index} = {index};\n"
            ));
    }
    source.push_str("export { ns_a, ns_b };\n");

    let output = unpack_raw(&source, &DecompileOptions::default())
        .expect("large detector raw split should unpack");
    let filenames: HashSet<_> = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();

    assert!(
        filenames.contains("ns_a.js") && filenames.contains("ns_b.js"),
        "detector raw output should preserve split cycle members instead of running merge cleanup"
    );
    assert!(
        output.modules.len() > 1000,
        "fixture should exercise large synthetic raw output, got {} modules",
        output.modules.len()
    );
}

#[test]
fn unpack_propagates_invalid_input_parse_errors() {
    let err = unpack(
        "const = ;",
        DecompileOptions {
            filename: "broken.js".to_string(),
            ..Default::default()
        },
    )
    .expect_err("invalid source should fail");

    assert!(
        err.to_string().contains("broken.js"),
        "error should include input filename: {err}"
    );
}

#[test]
fn unpack_preserves_typescript_single_file_fallback() {
    let output = unpack(
        "const value: number = 1;",
        DecompileOptions {
            filename: "input.ts".to_string(),
            ..Default::default()
        },
    )
    .expect("valid TypeScript should fall back to single-file decompile");

    assert_eq!(output.modules.len(), 1);
    assert_eq!(output.modules[0].0, "module.js");
    assert!(
        output.modules[0].1.contains("const value"),
        "expected TypeScript input to decompile, got: {}",
        output.modules[0].1
    );
}

#[test]
fn import_cycle_warnings_report_local_sccs() {
    let modules = vec![
        (
            "a.js".to_string(),
            r#"import { b } from "./b.js"; export const a = b;"#.to_string(),
        ),
        (
            "b.js".to_string(),
            r#"import { a } from "./a.js"; export const b = a;"#.to_string(),
        ),
        (
            "c.js".to_string(),
            r#"import { a } from "./a.js"; export const c = a;"#.to_string(),
        ),
    ];

    let warnings = collect_import_cycle_warnings(&modules);

    assert_eq!(warnings.len(), 1, "should report one SCC: {warnings:?}");
    assert_eq!(warnings[0].kind, UnpackWarningKind::ImportCycle);
    assert!(warnings[0].message.contains("2 modules"));
    assert!(warnings[0].message.contains("a.js"));
    assert!(warnings[0].message.contains("b.js"));
}

#[test]
fn import_cycle_warning_reports_a_real_deterministic_edge_witness() {
    // Alphabetical SCC member order is not an import path: the only cycle is
    // a -> c -> b -> a. The diagnostic must not present sorted membership as
    // if each adjacent pair were an edge.
    let modules = vec![
        (
            "c.js".to_string(),
            r#"import { b } from "./b.js"; export const c = b;"#.to_string(),
        ),
        (
            "a.js".to_string(),
            r#"import { c } from "./c.js"; export const a = c;"#.to_string(),
        ),
        (
            "b.js".to_string(),
            r#"import { a } from "./a.js"; export const b = a;"#.to_string(),
        ),
    ];

    let warnings = collect_import_cycle_warnings(&modules);

    assert_eq!(warnings.len(), 1, "should report one SCC: {warnings:?}");
    assert_eq!(
        warnings[0].message,
        "local import cycle across 3 modules; cycle witness: a.js -> c.js -> b.js -> a.js"
    );
}

#[test]
fn import_cycle_warning_closes_a_self_cycle_witness() {
    let modules = vec![(
        "self.js".to_string(),
        r#"import { value } from "./self.js"; export { value };"#.to_string(),
    )];

    let warnings = collect_import_cycle_warnings(&modules);

    assert_eq!(
        warnings.len(),
        1,
        "should report the self-cycle: {warnings:?}"
    );
    assert_eq!(
        warnings[0].message,
        "local import cycle across 1 modules; cycle witness: self.js -> self.js"
    );
}
