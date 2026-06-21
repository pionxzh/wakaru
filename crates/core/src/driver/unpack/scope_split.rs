//! Heuristic splitting of scope-hoisted modules inside detected bundles.
//!
//! Detector output modules can themselves be scope-hoisted concatenations
//! (esbuild/Bun style). When this aggressive-only pass is enabled, each
//! extracted module is re-examined and split further when the result still
//! resolves.

use std::collections::HashSet;

use crate::unpacker::{scope_hoist, UnpackResult, UnpackedModule};

pub(super) fn maybe_split_scope_hoisted_modules(
    result: UnpackResult,
    enabled: bool,
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
        match scope_hoist::split_scope_hoisted(&module.code) {
            Some(split) if split.modules.len() > 1 && has_nontrivial_scope_split_entry(&split) => {
                let parent_filename = module.filename.clone();
                let split_modules = namespace_scope_hoisted_split(&module, split.modules);
                let mut available_filenames = original_filenames.clone();
                available_filenames.remove(&parent_filename);
                available_filenames
                    .extend(split_modules.iter().map(|module| module.filename.clone()));
                if scope_split_imports_resolve(&split_modules, &available_filenames) {
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
        allow_cycle_premerge: result.allow_cycle_premerge && !did_split,
        format: result.format,
    }
}

fn has_nontrivial_scope_split_entry(split: &UnpackResult) -> bool {
    split
        .modules
        .iter()
        .find(|module| module.is_entry)
        .is_some_and(|module| module.code.contains("from \"./"))
}

fn namespace_scope_hoisted_split(
    parent: &UnpackedModule,
    split_modules: Vec<UnpackedModule>,
) -> Vec<UnpackedModule> {
    let (parent_dir, parent_stem, parent_basename) = split_parent_path_parts(&parent.filename);
    let child_dir = if parent_dir.is_empty() {
        parent_stem.clone()
    } else {
        format!("{parent_dir}/{parent_stem}")
    };
    let entry_import_dir = parent_stem;
    let child_filenames: HashSet<String> = split_modules
        .iter()
        .filter(|module| !module.is_entry)
        .map(|module| module.filename.clone())
        .collect();

    let mut modules = Vec::with_capacity(split_modules.len());
    for mut module in split_modules {
        if module.is_entry {
            module.id = parent.id.clone();
            module.is_entry = parent.is_entry;
            module.filename = parent.filename.clone();
            module.code =
                rewrite_scope_entry_imports(module.code, &entry_import_dir, &child_filenames);
        } else {
            module.id = format!("{}/{}", parent.id, module.id);
            module.filename = format!("{child_dir}/{}", module.filename);
            module.code =
                rewrite_scope_child_imports(module.code, &parent_basename, &child_filenames);
        }
        modules.push(module);
    }
    modules
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
            }],
            allow_cycle_premerge: true,
            format: BundleFormat::Webpack5,
        };

        let output = maybe_split_scope_hoisted_modules(result, false);

        assert_eq!(output.modules.len(), 1);
        assert_eq!(output.modules[0].id, "100");
        assert_eq!(output.modules[0].filename, "module-100.js");
        assert!(output.allow_cycle_premerge);
    }

    #[test]
    fn enabled_nested_scope_split_namespaces_child_modules() {
        let result = UnpackResult {
            modules: vec![UnpackedModule {
                id: "100".to_string(),
                is_entry: false,
                code: nested_scope_hoist_fixture(),
                filename: "module-100.js".to_string(),
            }],
            allow_cycle_premerge: true,
            format: BundleFormat::Webpack5,
        };

        let output = maybe_split_scope_hoisted_modules(result, true);
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
            !output.allow_cycle_premerge,
            "recursive scope split should disable later cycle premerge"
        );
    }

    #[test]
    fn namespace_scope_split_keeps_parent_filename_and_rewrites_entry_imports() {
        let parent = UnpackedModule {
            id: "11111".to_string(),
            is_entry: false,
            code: String::new(),
            filename: "module-11111.js".to_string(),
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
            },
            UnpackedModule {
                id: "chunk_other".to_string(),
                is_entry: false,
                code: r#"export const other = 1;
"#
                .to_string(),
                filename: "chunk_other.js".to_string(),
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
}
