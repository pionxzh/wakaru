//! Heuristic splitting of scope-hoisted modules inside detected bundles.
//!
//! Detector output modules can themselves be scope-hoisted concatenations
//! (esbuild/Bun style). When this aggressive-only pass is enabled, each
//! extracted module is re-examined and split further when the result still
//! resolves.

use std::collections::HashSet;

use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;

use super::super::io::parse_js;
use super::{recover_late_esm_from_factory_iifes, LateEsmRecoveryOptions};
use crate::rules::{apply_rules, RewriteLevel, RulePipelineOptions};
use crate::unpacker::{scope_hoist, GeneratedSourceMapPoint, UnpackResult, UnpackedModule};

pub(super) fn maybe_split_scope_hoisted_modules(
    result: UnpackResult,
    enabled: bool,
    render_mode: scope_hoist::ScopeHoistRenderMode,
) -> UnpackResult {
    if !enabled {
        return result;
    }

    let mut modules = Vec::new();
    let mut did_split = false;
    let original_filenames: HashSet<String> = result
        .modules
        .iter()
        .map(|module| module.filename.clone())
        .collect();

    for module in result.modules {
        match split_nested_scope_hoisted_module(&module, render_mode) {
            Some(split) => {
                let parent_filename = module.filename.clone();
                let split_modules = namespace_scope_hoisted_split(&module, split.modules);
                let mut available_filenames = original_filenames.clone();
                available_filenames.remove(&parent_filename);
                available_filenames
                    .extend(split_modules.iter().map(|module| module.filename.clone()));
                if scope_split_imports_resolve(&split_modules, &available_filenames)
                    && scope_split_modules_parse(&split_modules)
                {
                    did_split = true;
                    modules.extend(split_modules);
                } else {
                    modules.push(module);
                }
            }
            _ => modules.push(module),
        }
    }

    UnpackResult {
        modules,
        report_import_cycle_warnings: result.report_import_cycle_warnings && !did_split,
        format: result.format,
    }
}

fn split_nested_scope_hoisted_module(
    module: &UnpackedModule,
    render_mode: scope_hoist::ScopeHoistRenderMode,
) -> Option<UnpackResult> {
    let raw_split = scope_hoist::split_scope_hoisted_with_mode(
        &module.code,
        render_mode,
        scope_hoist::ScopeHoistSource::NestedModule,
    );
    if raw_split.as_ref().is_some_and(is_usable_nested_split) {
        return raw_split;
    }
    if module.generated_source_map.is_empty() {
        return None;
    }

    split_esm_recovered_scope_hoisted_module(&module.code, &module.filename, render_mode)
        .filter(is_usable_nested_split)
}

fn is_usable_nested_split(split: &UnpackResult) -> bool {
    split.modules.len() > 1 && has_nontrivial_scope_split_entry(split)
}

fn has_nontrivial_scope_split_entry(split: &UnpackResult) -> bool {
    split
        .modules
        .iter()
        .find(|module| module.is_entry)
        .is_some_and(|module| module.code.contains("from \"./"))
}

fn split_esm_recovered_scope_hoisted_module(
    source: &str,
    filename: &str,
    render_mode: scope_hoist::ScopeHoistRenderMode,
) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(source, filename, cm.clone()).ok()?;
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        apply_rules(
            &mut module,
            unresolved_mark,
            RulePipelineOptions::until("UnEsm"),
        );
        recover_late_esm_from_factory_iifes(
            &mut module,
            unresolved_mark,
            RewriteLevel::Standard,
            LateEsmRecoveryOptions {
                smart_rename: false,
                export_rename: false,
            },
        );
        scope_hoist::split_scope_hoisted_module_with_mode(
            &module,
            cm,
            render_mode,
            scope_hoist::ScopeHoistSource::NestedModule,
        )
    })
}

fn namespace_scope_hoisted_split(
    parent: &UnpackedModule,
    split_modules: Vec<UnpackedModule>,
) -> Vec<UnpackedModule> {
    let (_, parent_stem, parent_basename) = split_parent_path_parts(&parent.filename);
    let entry_import_dir = parent_stem;
    let child_filenames: HashSet<String> = split_modules
        .iter()
        .filter(|module| !module.is_entry)
        .map(|module| module.filename.clone())
        .collect();

    let mut modules = Vec::with_capacity(split_modules.len());
    for mut module in split_modules {
        let has_generated_map = !parent.generated_source_map.is_empty();
        let source_ranges = if has_generated_map {
            map_generated_ranges_to_source(&parent.generated_source_map, &module.source_ranges)
                .unwrap_or_default()
        } else {
            parent.source_ranges.clone()
        };
        let inspection_context_ranges = if has_generated_map {
            map_generated_ranges_to_source(
                &parent.generated_source_map,
                &module.inspection_context_ranges,
            )
            .unwrap_or_default()
        } else {
            // Copying the parent's whole provenance is a conservative module
            // fallback, but would falsely merge every nested context. Context
            // is useful only when its narrower ranges map back exactly.
            Vec::new()
        };
        module.source_ranges = source_ranges;
        module.inspection_context_ranges = inspection_context_ranges;
        module.source_input = parent.source_input.clone();
        module.generated_source_map.clear();
        if module.is_entry {
            module.id = parent.id.clone();
            module.is_entry = parent.is_entry;
            module.filename = parent.filename.clone();
            module.code =
                rewrite_scope_entry_imports(module.code, &entry_import_dir, &child_filenames);
        } else {
            module.id = format!("{}/{}", parent.id, module.id);
            module.filename = public_path_child_filename(&parent.filename, &module.filename);
            module.code =
                rewrite_scope_child_imports(module.code, &parent_basename, &child_filenames);
        }
        modules.push(module);
    }
    modules
}

/// Namespace a generated child beneath the public entry's stem.
///
/// Top-level public-path promotion and recursive scope splitting share this
/// layout so `assets/chunk.js` owns children below `assets/chunk/`.
pub(super) fn public_path_child_filename(
    public_entry_filename: &str,
    child_filename: &str,
) -> String {
    let (parent_dir, parent_stem, _) = split_parent_path_parts(public_entry_filename);
    if parent_dir.is_empty() {
        format!("{parent_stem}/{child_filename}")
    } else {
        format!("{parent_dir}/{parent_stem}/{child_filename}")
    }
}

fn map_generated_ranges_to_source(
    mappings: &[GeneratedSourceMapPoint],
    generated_ranges: &[(u32, u32)],
) -> Option<Vec<(u32, u32)>> {
    if mappings.is_empty() || generated_ranges.is_empty() {
        return None;
    }

    let mut source_ranges = Vec::new();
    for &(generated_start, generated_end) in generated_ranges {
        if generated_start >= generated_end {
            continue;
        }

        let start = mappings.partition_point(|point| point.generated_offset < generated_start);
        let end = mappings.partition_point(|point| point.generated_offset < generated_end);
        if start >= end {
            continue;
        }

        for idx in start..end {
            let source_start = mappings[idx].source_offset;
            // Only use the next mapping's source_offset as our end when it
            // falls inside the same generated range (idx + 1 < end).
            // Beyond that boundary the next mapping belongs to a different
            // child module and would over-claim provenance.
            let source_end = if idx + 1 < end {
                mappings[idx + 1].source_offset.max(source_start)
            } else {
                let generated_remaining = generated_end - mappings[idx].generated_offset;
                source_start.saturating_add(generated_remaining)
            };
            if source_start < source_end {
                source_ranges.push((source_start, source_end));
            }
        }
    }

    if source_ranges.is_empty() {
        return None;
    }
    Some(coalesce_ranges(source_ranges))
}

fn coalesce_ranges(mut ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    ranges.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (start, end) in ranges {
        match out.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => out.push((start, end)),
        }
    }
    out
}

fn split_parent_path_parts(filename: &str) -> (String, String, String) {
    let normalized = filename.replace('\\', "/");
    let (parent, basename) = normalized
        .rsplit_once('/')
        .map(|(parent, basename)| (parent.to_string(), basename))
        .unwrap_or_else(|| (String::new(), normalized.as_str()));
    let stem = basename
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .filter(|stem| !stem.is_empty())
        .unwrap_or("module")
        .to_string();
    (parent, stem, basename.to_string())
}

fn rewrite_scope_entry_imports(
    mut code: String,
    entry_import_dir: &str,
    child_filenames: &HashSet<String>,
) -> String {
    for child_filename in child_filenames {
        let old = format!("from \"./{child_filename}\"");
        let new = format!("from \"./{entry_import_dir}/{child_filename}\"");
        code = code.replace(&old, &new);
    }
    code
}

fn rewrite_scope_child_imports(
    mut code: String,
    parent_basename: &str,
    child_filenames: &HashSet<String>,
) -> String {
    let replacements = scan_static_relative_imports(&code)
        .into_iter()
        .filter_map(|import| {
            if import.specifier == "./entry.js" {
                return Some((import.start, import.end, format!("../{parent_basename}")));
            }

            let child_or_sibling = import.specifier.strip_prefix("./")?;
            if child_filenames.contains(child_or_sibling) {
                return None;
            }
            Some((import.start, import.end, format!("../{child_or_sibling}")))
        })
        .collect::<Vec<_>>();

    for (start, end, replacement) in replacements.into_iter().rev() {
        code.replace_range(start..end, &replacement);
    }
    code
}

fn scope_split_imports_resolve(
    modules: &[UnpackedModule],
    available_filenames: &HashSet<String>,
) -> bool {
    modules.iter().all(|module| {
        extract_static_relative_imports(&module.code)
            .into_iter()
            .all(|spec| {
                resolve_relative_module_filename(&module.filename, &spec)
                    .is_some_and(|filename| available_filenames.contains(&filename))
            })
    })
}

fn scope_split_modules_parse(modules: &[UnpackedModule]) -> bool {
    modules.iter().all(|module| {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            parse_js(&module.code, &module.filename, cm).is_ok()
        })
    })
}

fn extract_static_relative_imports(code: &str) -> Vec<String> {
    scan_static_relative_imports(code)
        .into_iter()
        .map(|import| import.specifier)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StaticRelativeImport {
    specifier: String,
    start: usize,
    end: usize,
}

fn scan_static_relative_imports(code: &str) -> Vec<StaticRelativeImport> {
    let mut imports = Vec::new();
    let bytes = code.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' => {
                index = skip_quoted(code, index);
                continue;
            }
            b'`' => {
                index = skip_template_literal(code, index);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = skip_line_comment(code, index + 2);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(code, index + 2);
                continue;
            }
            _ => {}
        }

        if starts_with_keyword(code, index, "from") {
            if let Some(import) = scan_quoted_specifier(code, skip_ascii_ws(code, index + 4)) {
                imports.push(import);
                index += 4;
                continue;
            }
        } else if starts_with_keyword(code, index, "import") {
            if let Some(import) = scan_quoted_specifier(code, skip_ascii_ws(code, index + 6)) {
                imports.push(import);
                index += 6;
                continue;
            }
        } else if starts_with_keyword(code, index, "require") {
            if let Some(import) = scan_require_specifier(code, index + 7) {
                imports.push(import);
                index += 7;
                continue;
            }
        }

        index += 1;
    }

    imports
}

fn starts_with_keyword(code: &str, index: usize, keyword: &str) -> bool {
    let after = index + keyword.len();
    if after > code.len() || !code.is_char_boundary(index) || !code.is_char_boundary(after) {
        return false;
    }

    code[index..].starts_with(keyword)
        && !code[..index]
            .bytes()
            .next_back()
            .is_some_and(is_js_ident_continue)
        && !code[index + keyword.len()..]
            .bytes()
            .next()
            .is_some_and(is_js_ident_continue)
}

fn is_js_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

fn skip_ascii_ws(code: &str, mut index: usize) -> usize {
    let bytes = code.as_bytes();
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn scan_require_specifier(code: &str, index: usize) -> Option<StaticRelativeImport> {
    let index = skip_ascii_ws(code, index);
    if code.as_bytes().get(index) != Some(&b'(') {
        return None;
    }
    scan_quoted_specifier(code, skip_ascii_ws(code, index + 1))
}

fn scan_quoted_specifier(code: &str, index: usize) -> Option<StaticRelativeImport> {
    let quote = *code.as_bytes().get(index)?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let start = index + 1;
    let end = find_quoted_end(code, index)?;
    let specifier = &code[start..end];
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return None;
    }
    Some(StaticRelativeImport {
        specifier: specifier.to_string(),
        start,
        end,
    })
}

fn find_quoted_end(code: &str, index: usize) -> Option<usize> {
    let quote = *code.as_bytes().get(index)?;
    let bytes = code.as_bytes();
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor = cursor.saturating_add(2),
            byte if byte == quote => return Some(cursor),
            _ => cursor += 1,
        }
    }
    None
}

fn skip_quoted(code: &str, index: usize) -> usize {
    find_quoted_end(code, index)
        .map(|end| end + 1)
        .unwrap_or(code.len())
}

fn skip_template_literal(code: &str, index: usize) -> usize {
    let bytes = code.as_bytes();
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor = cursor.saturating_add(2),
            b'`' => return cursor + 1,
            _ => cursor += 1,
        }
    }
    code.len()
}

fn skip_line_comment(code: &str, index: usize) -> usize {
    code[index..]
        .find('\n')
        .map(|offset| index + offset + 1)
        .unwrap_or(code.len())
}

fn skip_block_comment(code: &str, index: usize) -> usize {
    code[index..]
        .find("*/")
        .map(|offset| index + offset + 2)
        .unwrap_or(code.len())
}

fn resolve_relative_module_filename(current_filename: &str, specifier: &str) -> Option<String> {
    let normalized_current = current_filename.replace('\\', "/");
    let mut parts: Vec<&str> = normalized_current
        .rsplit_once('/')
        .map(|(parent, _)| parent.split('/').filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();

    for part in specifier.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }

    let mut resolved = parts.join("/");
    if !resolved.ends_with(".js") {
        resolved.push_str(".js");
    }
    Some(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unpacker::BundleFormat;

    #[test]
    fn disabled_nested_scope_split_preserves_detected_module() {
        let result = UnpackResult {
            modules: vec![UnpackedModule {
                id: "100".to_string(),
                is_entry: false,
                code: nested_scope_hoist_fixture(),
                filename: "module-100.js".to_string(),
                ..Default::default()
            }],
            report_import_cycle_warnings: true,
            format: BundleFormat::Webpack5,
        };

        let output = maybe_split_scope_hoisted_modules(
            result,
            false,
            scope_hoist::ScopeHoistRenderMode::Executable,
        );

        assert_eq!(output.modules.len(), 1);
        assert_eq!(output.modules[0].id, "100");
        assert_eq!(output.modules[0].filename, "module-100.js");
        assert!(output.report_import_cycle_warnings);
    }

    #[test]
    fn enabled_nested_scope_split_namespaces_child_modules() {
        let result = UnpackResult {
            modules: vec![UnpackedModule {
                id: "100".to_string(),
                is_entry: false,
                code: nested_scope_hoist_fixture(),
                filename: "module-100.js".to_string(),
                ..Default::default()
            }],
            report_import_cycle_warnings: true,
            format: BundleFormat::Webpack5,
        };

        let output = maybe_split_scope_hoisted_modules(
            result,
            true,
            scope_hoist::ScopeHoistRenderMode::Executable,
        );
        let names: HashSet<_> = output
            .modules
            .iter()
            .map(|module| module.filename.as_str())
            .collect();

        assert!(
            output.modules.len() > 1,
            "aggressive nested split should split fixture, got {:?}",
            names
        );
        assert!(names.contains("module-100.js"));
        assert!(
            names.iter().any(|name| name.starts_with("module-100/")),
            "child modules should be namespaced under parent filename: {:?}",
            names
        );
        assert!(
            !output.report_import_cycle_warnings,
            "recursive scope split output should opt out of cycle diagnostics"
        );
    }

    #[test]
    fn nested_inspection_split_keeps_cyclic_clusters_separate() {
        let split = |render_mode| {
            maybe_split_scope_hoisted_modules(
                UnpackResult {
                    modules: vec![UnpackedModule {
                        id: "100".to_string(),
                        is_entry: false,
                        code: nested_scope_hoist_cycle_fixture(),
                        filename: "module-100.js".to_string(),
                        ..Default::default()
                    }],
                    report_import_cycle_warnings: true,
                    format: BundleFormat::Webpack5,
                },
                true,
                render_mode,
            )
        };

        let safe = split(scope_hoist::ScopeHoistRenderMode::Executable);
        let inspection = split(scope_hoist::ScopeHoistRenderMode::Inspect);

        assert_eq!(safe.modules.len(), 5, "safe mode should merge the cycle");
        assert_eq!(
            inspection.modules.len(),
            6,
            "inspection mode should retain the finer cyclic clusters"
        );
    }

    #[test]
    fn namespace_scope_split_keeps_parent_filename_and_rewrites_entry_imports() {
        let parent = UnpackedModule {
            id: "11111".to_string(),
            is_entry: false,
            code: String::new(),
            filename: "module-11111.js".to_string(),
            ..Default::default()
        };
        let split_modules = vec![
            UnpackedModule {
                id: "entry".to_string(),
                is_entry: true,
                code: r#"import { value } from "./chunk_value.js";
console.log(value);
"#
                .to_string(),
                filename: "entry.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "chunk_value".to_string(),
                is_entry: false,
                code: r#"import { init } from "./entry.js";
import { other } from "./chunk_other.js";
import sibling from "./module-44444.js";
const siblingCjs = require("./module-44444.js");
const literal = 'require("./module-44444.js")';
// from "./module-44444.js";
export const value = init + 1;
"#
                .to_string(),
                filename: "chunk_value.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "chunk_other".to_string(),
                is_entry: false,
                code: r#"export const other = 1;
"#
                .to_string(),
                filename: "chunk_other.js".to_string(),
                ..Default::default()
            },
        ];

        let modules = namespace_scope_hoisted_split(&parent, split_modules);
        assert_eq!(modules[0].id, "11111");
        assert_eq!(modules[0].filename, "module-11111.js");
        assert!(
            modules[0]
                .code
                .contains(r#"from "./module-11111/chunk_value.js""#),
            "entry imports should target the namespaced child chunk:\n{}",
            modules[0].code
        );
        assert_eq!(modules[1].id, "11111/chunk_value");
        assert_eq!(modules[1].filename, "module-11111/chunk_value.js");
        assert!(
            modules[1].code.contains(r#"from "../module-11111.js""#),
            "child imports of split entry should target the preserved parent filename:\n{}",
            modules[1].code
        );
        assert!(
            modules[1].code.contains(r#"from "./chunk_other.js""#),
            "child-to-child imports should stay within the namespaced child dir:\n{}",
            modules[1].code
        );
        assert!(
            modules[1].code.contains(r#"from "../module-44444.js""#),
            "child imports of external sibling modules should point out of the child dir:\n{}",
            modules[1].code
        );
        assert!(
            modules[1].code.contains(r#"require("../module-44444.js")"#),
            "child require() calls of external sibling modules should point out of the child dir:\n{}",
            modules[1].code
        );
        assert!(
            modules[1]
                .code
                .contains(r#"const literal = 'require("./module-44444.js")';"#),
            "import-looking text in string literals should not be rewritten:\n{}",
            modules[1].code
        );
        assert!(
            modules[1].code.contains(r#"// from "./module-44444.js";"#),
            "import-looking text in comments should not be rewritten:\n{}",
            modules[1].code
        );

        let mut available: HashSet<String> = modules
            .iter()
            .map(|module| module.filename.clone())
            .collect();
        available.insert("module-44444.js".to_string());
        assert!(scope_split_imports_resolve(&modules, &available));

        let missing_entry = HashSet::from(["module-11111/chunk_value.js".to_string()]);
        assert!(!scope_split_imports_resolve(&modules, &missing_entry));
    }

    #[test]
    fn namespace_scope_split_maps_inspection_context_separately() {
        let parent = UnpackedModule {
            id: "100".to_string(),
            filename: "module-100.js".to_string(),
            source_input: "bundle.js".to_string(),
            generated_source_map: vec![
                GeneratedSourceMapPoint {
                    generated_offset: 0,
                    source_offset: 100,
                },
                GeneratedSourceMapPoint {
                    generated_offset: 10,
                    source_offset: 110,
                },
                GeneratedSourceMapPoint {
                    generated_offset: 20,
                    source_offset: 120,
                },
                GeneratedSourceMapPoint {
                    generated_offset: 30,
                    source_offset: 130,
                },
            ],
            ..Default::default()
        };
        let split_modules = vec![
            UnpackedModule {
                id: "left".to_string(),
                filename: "chunk_left.js".to_string(),
                source_ranges: vec![(10, 20)],
                inspection_context_ranges: vec![(0, 30)],
                ..Default::default()
            },
            UnpackedModule {
                id: "right".to_string(),
                filename: "chunk_right.js".to_string(),
                source_ranges: vec![(20, 30)],
                inspection_context_ranges: vec![(0, 30)],
                ..Default::default()
            },
        ];

        let modules = namespace_scope_hoisted_split(&parent, split_modules);
        assert_eq!(modules[0].source_ranges, vec![(110, 120)]);
        assert_eq!(modules[1].source_ranges, vec![(120, 130)]);
        assert_eq!(modules[0].inspection_context_ranges, vec![(100, 130)]);
        assert_eq!(
            modules[0].inspection_context_ranges,
            modules[1].inspection_context_ranges
        );
        assert!(modules
            .iter()
            .all(|module| module.generated_source_map.is_empty()
                && module.source_input == "bundle.js"));

        let without_map = namespace_scope_hoisted_split(
            &UnpackedModule {
                id: "200".to_string(),
                filename: "module-200.js".to_string(),
                source_ranges: vec![(500, 600)],
                ..Default::default()
            },
            vec![UnpackedModule {
                id: "child".to_string(),
                filename: "chunk_child.js".to_string(),
                source_ranges: vec![(10, 20)],
                inspection_context_ranges: vec![(0, 30)],
                ..Default::default()
            }],
        );
        assert_eq!(without_map[0].source_ranges, vec![(500, 600)]);
        assert!(without_map[0].inspection_context_ranges.is_empty());
    }

    #[test]
    fn scope_split_parse_guard_rejects_invalid_child_modules() {
        let modules = vec![
            UnpackedModule {
                id: "entry".to_string(),
                is_entry: true,
                code: r#"import { value } from "./chunk_value.js";
console.log(value);
"#
                .to_string(),
                filename: "entry.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "chunk_value".to_string(),
                is_entry: false,
                code: "const = ;".to_string(),
                filename: "chunk_value.js".to_string(),
                ..Default::default()
            },
        ];

        assert!(
            !scope_split_modules_parse(&modules),
            "invalid nested split output should be rejected before CLI unpack fails"
        );
    }

    fn nested_scope_hoist_fixture() -> String {
        r#"
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
            export { result };
        "#
        .to_string()
    }

    fn nested_scope_hoist_cycle_fixture() -> String {
        r#"
            class A {}
            const x1 = 1; function f1() { return x1; }
            const x2 = 2; function f2() { return x2; }
            const x3 = 3; function f3() { return x3; }
            const x4 = 4; function f4() { return x4; }
            function make() { return new A(); }
            const result = make();
            console.log(result, f1(), f2(), f3(), f4());
            export { result };
        "#
        .to_string()
    }
}
