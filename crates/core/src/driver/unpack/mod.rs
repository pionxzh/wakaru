use anyhow::{anyhow, Result};
use rayon::prelude::*;
use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::visit::VisitMutWith;

use super::io::{parse_js, print_js};
use super::single_file::decompile;
use super::types::{DecompileOptions, UnpackInput, UnpackOutput, UnpackWarning, UnpackWarningKind};
#[cfg(test)]
use super::unpack_cleanup::hoist_late_runtime_helpers;
use super::unpack_cycles::merge_import_cycles;
#[cfg(test)]
use super::unpack_cycles::{
    collect_import_cycle_warnings, scan_local_import_dependencies, unsafe_merge_member_reason,
};
use crate::rules::{
    apply_rules, ArrowFunction, ArrowReturn, RewriteLevel, RulePipelineOptions, SmartRename, UnEsm,
    UnExportRename, UnIife,
};
use crate::unpacker::{scope_hoist, try_unpack_bundle, webpack5, UnpackResult, UnpackedModule};

mod filename_recovery;
mod merge;
mod phases;
mod scope_split;

use merge::{
    emit_raw_modules_with_numeric_rewrites, prepare_multi_source_modules, MultiSourceModule,
    NumericRewritePlan, PreparedUnpackModule,
};
#[cfg(test)]
use phases::unpack_multi_module;
use phases::unpack_multi_module_with_plan;
use scope_split::maybe_split_scope_hoisted_modules;

pub fn unpack(source: &str, options: DecompileOptions) -> Result<UnpackOutput> {
    let span = tracing::info_span!("unpack");
    let _enter = span.enter();

    match detect_bundle(source, &options.filename)? {
        Some(result) => {
            let result = maybe_split_scope_hoisted_modules(result, options.heuristic_split);
            unpack_unpack_result(result, options)
        }
        None if options.heuristic_split => match scope_hoist::split_scope_hoisted(source) {
            Some(result) if result.modules.len() > 1 => {
                let mut opts = options.clone();
                opts.dead_code_elimination = false;
                unpack_unpack_result(result, opts)
            }
            _ => {
                let output = decompile(source, options)?;
                Ok(UnpackOutput {
                    modules: vec![("module.js".to_string(), output.code)],
                    warnings: output.warnings,
                })
            }
        },
        None => {
            let output = decompile(source, options)?;
            Ok(UnpackOutput {
                modules: vec![("module.js".to_string(), output.code)],
                warnings: output.warnings,
            })
        }
    }
}

pub fn unpack_files(
    mut inputs: Vec<UnpackInput>,
    mut options: DecompileOptions,
) -> Result<UnpackOutput> {
    if inputs.is_empty() {
        return Err(anyhow!("at least one input file is required"));
    }

    if inputs.len() == 1 {
        let input = inputs.pop().expect("checked input length");
        options.filename = input.filename;
        return unpack(&input.source, options);
    }

    let span = tracing::info_span!("unpack_files", count = inputs.len());
    let _enter = span.enter();

    let mut modules = Vec::new();
    for input in inputs {
        match detect_bundle(&input.source, &input.filename)? {
            Some(result) => {
                let result = maybe_split_scope_hoisted_modules(result, options.heuristic_split);
                let chunk_ids = webpack5::detect_chunk_ids(&input.source);
                let input_filename = input.filename.clone();
                let allow_cycle_premerge = result.allow_cycle_premerge;
                modules.extend(result.modules.into_iter().map(|module| {
                    MultiSourceModule::detected(
                        module,
                        chunk_ids.clone(),
                        input_filename.clone(),
                        allow_cycle_premerge,
                    )
                }))
            }
            None if options.heuristic_split => {
                match scope_hoist::split_scope_hoisted(&input.source) {
                    Some(result) if result.modules.len() > 1 => {
                        modules.extend(result.modules.into_iter().map(MultiSourceModule::fallback))
                    }
                    _ => modules.push(MultiSourceModule::fallback(
                        crate::unpacker::UnpackedModule {
                            id: input.filename.clone(),
                            is_entry: false,
                            code: input.source,
                            filename: filename_for_fallback_input(&input.filename),
                        },
                    )),
                }
            }
            None => modules.push(MultiSourceModule::fallback(
                crate::unpacker::UnpackedModule {
                    id: input.filename.clone(),
                    is_entry: false,
                    code: input.source,
                    filename: filename_for_fallback_input(&input.filename),
                },
            )),
        }
    }

    if modules.is_empty() {
        return Err(anyhow!("no modules were extracted from input files"));
    }

    let (modules, numeric_rewrite_plan) = prepare_multi_source_modules(modules);
    unpack_multi_module_with_plan(modules, numeric_rewrite_plan, options)
}

/// Unpack a bundle without running the decompiler rule pipeline.
///
/// This returns raw module output after detector-specific extraction and
/// bundler-coupled normalization. Cross-module analysis and the normal
/// decompile rule pipeline are skipped.
pub fn unpack_raw(source: &str, options: &DecompileOptions) -> Result<UnpackOutput> {
    let result = detect_bundle_raw(source, &options.filename)?
        .map(|result| {
            (
                maybe_split_scope_hoisted_modules(result, options.heuristic_split),
                false,
            )
        })
        .or_else(|| {
            if options.heuristic_split {
                let r = scope_hoist::split_scope_hoisted(source)?;
                if r.modules.len() > 1 {
                    Some((r, true))
                } else {
                    None
                }
            } else {
                None
            }
        });
    match result {
        Some((result, normalize_for_runnable_split)) => {
            let (modules, warnings) = if normalize_for_runnable_split {
                // Heuristic scope-hoisted fallback does not get the esbuild
                // detector's bundler-specific cleanup, so keep the narrow
                // runnable normalization it still relies on.
                let (modules, warnings) = {
                    if should_merge_raw_import_cycles(&result.modules) {
                        let span = tracing::info_span!("raw_merge_import_cycles");
                        let _enter = span.enter();
                        merge_import_cycles(result.modules)
                    } else {
                        (result.modules, Vec::new())
                    }
                };
                let normalized: Vec<_> = modules
                    .into_par_iter()
                    .map(|module| {
                        match normalize_raw_unpacked_module(&module.code, &module.filename) {
                            Ok(normalized) => ((module.filename, normalized), None),
                            Err(e) => {
                                let warning = UnpackWarning::new(
                                    module.filename.clone(),
                                    UnpackWarningKind::RawNormalizationFailed,
                                    format!(
                                        "raw normalization failed, preserving unparsed code: {e}"
                                    ),
                                );
                                ((module.filename, module.code), Some(warning))
                            }
                        }
                    })
                    .collect();
                let mut output_modules = Vec::with_capacity(normalized.len());
                let mut output_warnings = if options.diagnostics {
                    warnings
                } else {
                    Vec::new()
                };
                for (module, warning) in normalized {
                    if options.diagnostics {
                        if let Some(warning) = warning {
                            output_warnings.push(warning);
                        }
                    }
                    output_modules.push(module);
                }
                (output_modules, output_warnings)
            } else {
                (
                    result
                        .modules
                        .into_iter()
                        .map(|module| (module.filename, module.code))
                        .collect(),
                    Vec::new(),
                )
            };
            Ok(UnpackOutput { modules, warnings })
        }
        None => Ok(UnpackOutput {
            modules: vec![("module.js".to_string(), source.to_string())],
            warnings: Vec::new(),
        }),
    }
}

pub fn unpack_files_raw(
    mut inputs: Vec<UnpackInput>,
    options: &DecompileOptions,
) -> Result<UnpackOutput> {
    if inputs.is_empty() {
        return Err(anyhow!("at least one input file is required"));
    }

    if inputs.len() == 1 {
        let input = inputs.pop().expect("checked input length");
        let mut opts = options.clone();
        opts.filename = input.filename;
        return unpack_raw(&input.source, &opts);
    }

    let mut modules = Vec::new();

    for input in inputs {
        let result = detect_bundle_raw(&input.source, &input.filename)?.or_else(|| {
            if options.heuristic_split {
                let r = scope_hoist::split_scope_hoisted(&input.source)?;
                if r.modules.len() > 1 {
                    Some(r)
                } else {
                    None
                }
            } else {
                None
            }
        });

        match result {
            Some(result) => {
                let result = maybe_split_scope_hoisted_modules(result, options.heuristic_split);
                let chunk_ids = webpack5::detect_chunk_ids(&input.source);
                let allow_cycle_premerge = result.allow_cycle_premerge;
                modules.extend(result.modules.into_iter().map(|module| {
                    MultiSourceModule::detected(
                        module,
                        chunk_ids.clone(),
                        input.filename.clone(),
                        allow_cycle_premerge,
                    )
                }));
            }
            None => modules.push(MultiSourceModule::fallback(UnpackedModule {
                id: input.filename.clone(),
                is_entry: false,
                code: input.source.to_string(),
                filename: filename_for_fallback_input(&input.filename),
            })),
        }
    }

    let (modules, numeric_rewrite_plan) = prepare_multi_source_modules(modules);
    emit_raw_modules_with_numeric_rewrites(modules, numeric_rewrite_plan)
}

fn filename_for_fallback_input(filename: &str) -> String {
    let path = std::path::Path::new(filename);
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("module.js")
        .to_string()
}

pub(super) fn detect_bundle(source: &str, filename: &str) -> Result<Option<UnpackResult>> {
    let span = tracing::info_span!("detect_bundle");
    let _enter = span.enter();

    match try_unpack_bundle(source) {
        Ok(result) => Ok(result),
        Err(bundle_parse_error) => {
            // Bundle detection intentionally parses only ES/JSX. Preserve the
            // single-file fallback for valid inputs that use filename-driven
            // syntax such as TypeScript. That means a second parse here is
            // intentional: it distinguishes unsupported bundle syntax from
            // genuinely invalid input.
            let input_parse_result = GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                parse_js(source, filename, cm)
            });
            match input_parse_result {
                Ok(_) => Ok(None),
                Err(input_parse_error) => Err(anyhow!(
                    "{input_parse_error}; bundle detection also failed: {bundle_parse_error}"
                )),
            }
        }
    }
}

fn detect_bundle_raw(source: &str, filename: &str) -> Result<Option<UnpackResult>> {
    match crate::unpacker::try_unpack_bundle_raw(source) {
        Ok(result) => Ok(result),
        Err(bundle_parse_error) => {
            let input_parse_result = GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                parse_js(source, filename, cm)
            });
            match input_parse_result {
                Ok(_) => Ok(None),
                Err(input_parse_error) => Err(anyhow!(
                    "{input_parse_error}; bundle detection also failed: {bundle_parse_error}"
                )),
            }
        }
    }
}

fn normalize_raw_unpacked_module(source: &str, filename: &str) -> Result<String> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(source, filename, cm.clone())?;
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        module.visit_mut_with(&mut UnEsm::new(unresolved_mark, RewriteLevel::Standard));
        recover_late_esm_from_factory_iifes(
            &mut module,
            unresolved_mark,
            RewriteLevel::Standard,
            LateEsmRecoveryOptions {
                smart_rename: false,
                export_rename: false,
            },
        );
        module.visit_mut_with(&mut fixer(None));
        print_js(&module, cm)
    })
}

fn recover_late_esm_from_factory_iifes(
    module: &mut Module,
    unresolved_mark: Mark,
    level: RewriteLevel,
    options: LateEsmRecoveryOptions,
) {
    module.visit_mut_with(&mut ArrowFunction);
    module.visit_mut_with(&mut ArrowReturn);
    module.visit_mut_with(&mut UnIife::new(level));
    apply_rules(
        module,
        unresolved_mark,
        RulePipelineOptions::between("UnCurlyBraces", "UnEsm").with_rewrite_level(level),
    );
    if options.smart_rename {
        module.visit_mut_with(&mut SmartRename::new(unresolved_mark));
    }
    if options.export_rename {
        module.visit_mut_with(&mut UnExportRename);
    }
    module.visit_mut_with(&mut ArrowReturn);
}

#[derive(Clone, Copy)]
struct LateEsmRecoveryOptions {
    smart_rename: bool,
    export_rename: bool,
}

impl Default for LateEsmRecoveryOptions {
    fn default() -> Self {
        Self {
            smart_rename: true,
            export_rename: true,
        }
    }
}

fn unpack_unpack_result(result: UnpackResult, options: DecompileOptions) -> Result<UnpackOutput> {
    let allow_cycle_premerge = result.allow_cycle_premerge;
    let modules = result
        .modules
        .into_iter()
        .map(|module| PreparedUnpackModule::with_cycle_premerge(module, allow_cycle_premerge))
        .collect();
    unpack_multi_module_with_plan(modules, NumericRewritePlan::default(), options)
}

fn should_merge_raw_import_cycles(_modules: &[UnpackedModule]) -> bool {
    // Keep the raw merge hook available, but disabled for now. ESM cycles are
    // often valid, and the previous repair could undo recovered module
    // boundaries before users had a chance to inspect raw output.
    false
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;
    use crate::unpacker::UnpackedModule;

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
        }];
        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("unparseable extracted modules should be preserved as raw code");
        assert_eq!(
            output.modules,
            vec![("module-1.js".to_string(), "const = ;".to_string())]
        );
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
    fn merge_import_cycles_drops_internal_imports_and_retargets_consumers() {
        let modules = vec![
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code: r#"import { b } from "./b.js"; export const a = b + 1;"#.to_string(),
                filename: "a.js".to_string(),
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code: r#"import { a } from "./a.js"; export const b = a + 1;"#.to_string(),
                filename: "b.js".to_string(),
            },
            UnpackedModule {
                id: "c".to_string(),
                is_entry: false,
                code: r#"import { b } from "./b.js"; export const c = b;"#.to_string(),
                filename: "c.js".to_string(),
            },
        ];

        let (merged, warnings) = merge_import_cycles(modules);

        assert!(
            warnings.is_empty(),
            "successful cycle repair should not surface as stderr warnings: {warnings:?}"
        );
        assert_eq!(merged.len(), 2);
        let a = merged
            .iter()
            .find(|module| module.filename == "a.js")
            .expect("cycle should merge into first module");
        assert!(
            !a.code.contains("from \"./b.js\"") && a.code.contains("export const b"),
            "merged cycle should drop internal imports and retain member code:\n{}",
            a.code
        );
        let c = merged
            .iter()
            .find(|module| module.filename == "c.js")
            .expect("consumer should remain separate");
        assert!(
            c.code.contains("from \"./a.js\""),
            "consumer should retarget imports to merged representative:\n{}",
            c.code
        );
    }

    #[test]
    fn merge_import_cycles_does_not_reprint_unrelated_modules() {
        let untouched_code = "const untouched = 1   ;";
        let modules = vec![
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code: r#"import { b } from "./b.js"; export const a = b + 1;"#.to_string(),
                filename: "a.js".to_string(),
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code: r#"import { a } from "./a.js"; export const b = a + 1;"#.to_string(),
                filename: "b.js".to_string(),
            },
            UnpackedModule {
                id: "d".to_string(),
                is_entry: false,
                code: untouched_code.to_string(),
                filename: "d.js".to_string(),
            },
        ];

        let (merged, warnings) = merge_import_cycles(modules);

        assert!(
            warnings.is_empty(),
            "successful cycle repair should not surface as stderr warnings: {warnings:?}"
        );
        let unrelated = merged
            .iter()
            .find(|module| module.filename == "d.js")
            .expect("unrelated module should remain");
        assert_eq!(unrelated.code, untouched_code);
    }

    #[test]
    fn merge_import_cycles_dedups_external_imports_before_safety_check() {
        let modules = vec![
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code: r#"import { shared } from "./x.js"; import { b } from "./b.js"; export const a = b + shared;"#
                    .to_string(),
                filename: "a.js".to_string(),
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code: r#"import { shared } from "./x.js"; import { a } from "./a.js"; export const b = a + shared;"#
                    .to_string(),
                filename: "b.js".to_string(),
            },
        ];

        let (merged, warnings) = merge_import_cycles(modules);

        assert_eq!(merged.len(), 1, "warnings: {warnings:?}");
        assert!(
            warnings.is_empty(),
            "duplicate external imports should not block a safe merge or emit stderr warnings: {:?}",
            warnings
        );
        let a = &merged[0];
        assert_eq!(a.filename, "a.js");
        assert_eq!(
            a.code.matches("from \"./x.js\"").count(),
            1,
            "merged cycle should deduplicate external imports:\n{}",
            a.code
        );
        assert!(
            !a.code.contains("from \"./b.js\"") && a.code.contains("export const b"),
            "merged cycle should drop internal imports and retain member code:\n{}",
            a.code
        );
    }

    #[test]
    fn merge_import_cycles_dedups_redundant_named_exports() {
        let modules = vec![
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code: r#"import { b } from "./b.js"; export function f() { return b; }"#
                    .to_string(),
                filename: "a.js".to_string(),
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code: r#"import { f } from "./a.js"; export const b = 1; export { f };"#
                    .to_string(),
                filename: "b.js".to_string(),
            },
        ];

        let (merged, warnings) = merge_import_cycles(modules);

        assert_eq!(merged.len(), 1, "warnings: {warnings:?}");
        let a = &merged[0];
        assert!(
            a.code.contains("export function f"),
            "merged cycle should keep the declaration export:\n{}",
            a.code
        );
        assert!(
            !a.code.contains("export { f"),
            "merged cycle should remove the redundant named export:\n{}",
            a.code
        );
    }

    #[test]
    fn hoist_late_runtime_helpers_moves_helper_defs_before_side_effects() {
        let input = r#"
setup();
result = helper(value);
const { defineProperty } = Object;
var helper = (target) => defineProperty({}, "x", { value: target });
let cache;
function setup() {}
consumer = wrap(ns);
export var ns = {};
Object.defineProperty(ns, "value", { enumerable: true, get: () => value });
export { helper, cache };
"#;

        let output = GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = parse_js(input, "fixture.js", cm.clone()).expect("input parses");
            hoist_late_runtime_helpers(&mut module);
            print_js(&module, cm).expect("output prints")
        });

        let define_property = output
            .find("const { defineProperty")
            .expect("object destructuring helper should remain");
        let helper = output
            .find("var helper")
            .expect("helper declaration should remain");
        let cache = output
            .find("let cache")
            .expect("state declaration should remain");
        let call = output.find("result = helper").expect("call should remain");
        let namespace = output
            .find("export var ns")
            .expect("namespace export should remain");
        let namespace_getter = output
            .find("Object.defineProperty(ns")
            .expect("namespace getter should remain");
        let namespace_use = output.find("consumer = wrap").expect("use should remain");

        assert!(
            define_property < call && helper < call && cache < call,
            "late helper declarations should move before side effects:\n{output}"
        );
        assert!(
            namespace < namespace_use && namespace_getter < namespace_use,
            "late namespace export setup should move before side effects:\n{output}"
        );
    }

    #[test]
    fn merge_import_cycles_skips_duplicate_declaration_merges() {
        let modules = vec![
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code:
                    r#"import { b } from "./b.js"; const shared = 1; export const a = b + shared;"#
                        .to_string(),
                filename: "a.js".to_string(),
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code:
                    r#"import { a } from "./a.js"; const shared = 2; export const b = a + shared;"#
                        .to_string(),
                filename: "b.js".to_string(),
            },
        ];

        let (merged, warnings) = merge_import_cycles(modules);

        assert_eq!(merged.len(), 2, "unsafe cycles should stay split");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].message.contains("not merged")
                && warnings[0].message.contains("duplicate declarations"),
            "warning should explain why the cycle stayed split: {:?}",
            warnings
        );
        let a = merged
            .iter()
            .find(|module| module.filename == "a.js")
            .expect("a.js should remain separate");
        assert!(
            a.code.contains("from \"./b.js\""),
            "skipped cycle should preserve original imports:\n{}",
            a.code
        );
    }

    #[test]
    fn merge_import_cycles_skips_large_components() {
        let modules: Vec<UnpackedModule> = (0..33)
            .map(|index| {
                let next = (index + 1) % 33;
                UnpackedModule {
                    id: format!("m{index}"),
                    is_entry: false,
                    code: format!(
                        r#"import {{ v{next} }} from "./m{next}.js"; export const v{index} = v{next} + {index};"#
                    ),
                    filename: format!("m{index}.js"),
                }
            })
            .collect();

        let (merged, warnings) = merge_import_cycles(modules);

        assert_eq!(merged.len(), 33, "large cycles should stay split");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].message.contains("not merged")
                && warnings[0].message.contains("large-cycle merge limit"),
            "warning should explain why the large cycle stayed split: {:?}",
            warnings
        );
    }

    #[test]
    fn fast_cycle_preflight_allows_duplicate_var_declarations() {
        let modules = [
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code: r#"import { b } from "./b.js"; var shared = 1; export const a = b + shared;"#
                    .to_string(),
                filename: "a.js".to_string(),
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code: r#"import { a } from "./a.js"; var shared = 2; export const b = a + shared;"#
                    .to_string(),
                filename: "b.js".to_string(),
            },
        ];
        let module_by_filename: HashMap<String, &UnpackedModule> = modules
            .iter()
            .map(|module| (module.filename.clone(), module))
            .collect();
        let module_names: HashSet<String> = modules
            .iter()
            .map(|module| module.filename.clone())
            .collect();
        let members = vec!["a.js".to_string(), "b.js".to_string()];
        let member_set: HashSet<String> = members.iter().cloned().collect();

        assert!(
            unsafe_merge_member_reason(&members, &module_by_filename, &module_names, &member_set)
                .is_none(),
            "generated duplicate vars should not block the large-cycle fast preflight"
        );
    }
}
