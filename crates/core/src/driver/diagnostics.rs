use crate::collections::HashMap;

use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::ModuleItem;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;

use super::io::{parse_js_with_recovery, parse_script_with_recovery, ParseDiagnostic};
use super::types::{UnpackWarning, UnpackWarningKind};

pub(super) fn collect_tdz_warnings(
    module: &swc_core::ecma::ast::Module,
    filename: &str,
) -> Vec<UnpackWarning> {
    crate::tdz_check::check_tdz(module)
        .into_iter()
        .map(|v| {
            UnpackWarning::new(
                filename,
                UnpackWarningKind::TdzViolation,
                format!("reference to `{}` before declaration", v.name),
            )
        })
        .collect()
}

/// Report export names that an ES module output still accesses through
/// `exports`. This reads the final pipeline AST instead of re-parsing the
/// emitted code, so it also runs without `--diagnostics`. Only the free
/// `exports` and `module` globals matter, and rules leave those with the
/// resolver's `unresolved_mark`.
pub(super) fn collect_commonjs_export_residual_warnings(
    module: &swc_core::ecma::ast::Module,
    unresolved_mark: Mark,
    filename: &str,
) -> Vec<UnpackWarning> {
    let names =
        crate::commonjs_export_residual::unrecovered_commonjs_export_names(module, unresolved_mark);
    if names.is_empty() {
        return Vec::new();
    }
    let list = names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    vec![UnpackWarning::new(
        filename,
        UnpackWarningKind::CommonJsExportUnrecovered,
        format!(
            "ES module output still accesses CommonJS exports {list}; these accesses throw a ReferenceError when they run"
        ),
    )]
}

pub(super) fn collect_input_parse_warnings(errors: &[ParseDiagnostic]) -> Vec<UnpackWarning> {
    // Source locations identify occurrences, not distinct parser conditions.
    // Keep the first-seen signature order while collapsing repeated conditions
    // within one parsed file.
    let mut group_indexes: HashMap<(&str, &str), usize> = HashMap::default();
    let mut groups: Vec<(&ParseDiagnostic, usize)> = Vec::new();

    for error in errors {
        let signature = (error.filename.as_str(), error.message.as_str());
        if let Some(index) = group_indexes.get(&signature).copied() {
            groups[index].1 += 1;
        } else {
            group_indexes.insert(signature, groups.len());
            groups.push((error, 1));
        }
    }

    groups
        .into_iter()
        .map(|(first, occurrences)| {
            let message = if occurrences == 1 {
                format!("input parse recovered from parser error: {first}")
            } else {
                format!(
                    "input parse recovered from repeated parser error {} ({occurrences} occurrences; first at {}:{}:{})",
                    first.message, first.filename, first.line, first.column
                )
            };
            UnpackWarning::new(
                &first.filename,
                UnpackWarningKind::InputParseRecovered,
                message,
            )
        })
        .collect()
}

pub(super) fn collect_duplicate_declaration_warnings(
    module: &swc_core::ecma::ast::Module,
    filename: &str,
) -> Vec<UnpackWarning> {
    crate::output_validate::conflicting_lexical_declaration_names(module)
        .into_iter()
        .map(|name| {
            UnpackWarning::new(
                filename,
                UnpackWarningKind::DuplicateDeclaration,
                format!("duplicate lexical declaration `{name}`"),
            )
        })
        .collect()
}

/// Validate the text users receive, then resolve that emitted program from
/// scratch before running identity-sensitive diagnostics. Transform rules can
/// legally change lexical scope without rebuilding every pre-transform
/// `SyntaxContext`; those internal contexts must not become user warnings.
pub(super) fn collect_output_diagnostics(code: &str, filename: &str) -> Vec<UnpackWarning> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        match parse_js_with_recovery(code, filename, cm) {
            Ok(parsed) if parsed.recoverable_errors.is_empty() => {
                collect_resolved_output_warnings(parsed.module, filename)
            }
            Ok(parsed)
                if parsed
                    .module
                    .body
                    .iter()
                    .any(|item| matches!(item, ModuleItem::ModuleDecl(_))) =>
            {
                output_parse_warnings(parsed.recoverable_errors, filename)
            }
            Ok(parsed) => match parse_script_with_recovery(code, filename, Default::default()) {
                Ok(script) if script.recoverable_errors.is_empty() => {
                    collect_resolved_output_warnings(script.module, filename)
                }
                Ok(script) => output_parse_warnings(script.recoverable_errors, filename),
                Err(_) => output_parse_warnings(parsed.recoverable_errors, filename),
            },
            Err(module_error) => {
                match parse_script_with_recovery(code, filename, Default::default()) {
                    Ok(script) if script.recoverable_errors.is_empty() => {
                        collect_resolved_output_warnings(script.module, filename)
                    }
                    Ok(script) => output_parse_warnings(script.recoverable_errors, filename),
                    Err(_) => vec![UnpackWarning::new(
                        filename,
                        UnpackWarningKind::OutputParseFailed,
                        format!("emitted output failed to parse: {module_error}"),
                    )],
                }
            }
        }
    })
}

fn collect_resolved_output_warnings(
    mut module: swc_core::ecma::ast::Module,
    filename: &str,
) -> Vec<UnpackWarning> {
    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

    let mut warnings = collect_tdz_warnings(&module, filename);
    warnings.extend(collect_duplicate_declaration_warnings(&module, filename));
    warnings
}

fn output_parse_warnings(errors: Vec<ParseDiagnostic>, filename: &str) -> Vec<UnpackWarning> {
    errors
        .into_iter()
        .map(|error| {
            UnpackWarning::new(
                filename,
                UnpackWarningKind::OutputParseRecovered,
                format!("emitted output parse recovered from parser error: {error}"),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emitted_var_lexical_conflicts_are_errors_in_either_order() {
        for source in [
            "export const value = 1; export var value = 2;",
            "export var value = 1; export const value = 2;",
            "let value; if (ok) { var value; }",
            "{ const value = 1; if (ok) { var value; } }",
            "function f() { const value = 1; var value; }",
            "var f = () => { const value = 1; var value; };",
            "for (let value of values) { var value; }",
            "switch (tag) { case 0: const value = 1; break; case 1: var value; }",
            "const { value } = source; var value;",
            "var value; class value {}",
        ] {
            let warnings = collect_output_diagnostics(source, "entry.js");
            assert!(
                warnings
                    .iter()
                    .any(|warning| warning.kind == UnpackWarningKind::DuplicateDeclaration),
                "missed conflict: {source}\n{warnings:?}"
            );
        }
    }

    #[test]
    fn legal_var_redeclarations_and_nested_shadowing_are_not_errors() {
        for source in [
            "var value; var value;",
            "var value; if (ok) { var value; }",
            "var value; { const value = 1; }",
            "const value = 1; function f() { var value; }",
            "function f(value) { var value; }",
            "function f(value, value) {}",
            "function value() {} function value() {}",
            "var value; function value() {}",
            "const value = class value {};",
        ] {
            let warnings = collect_output_diagnostics(source, "entry.js");
            assert!(
                !warnings
                    .iter()
                    .any(|warning| warning.kind == UnpackWarningKind::DuplicateDeclaration),
                "false conflict: {source}\n{warnings:?}"
            );
        }
    }

    fn parse_diagnostic(line: usize, message: &str) -> ParseDiagnostic {
        ParseDiagnostic {
            filename: "classic-script.js".to_string(),
            line,
            column: 1,
            message: message.to_string(),
        }
    }

    #[test]
    fn input_parse_warning_coalescing_preserves_signature_order() {
        let warnings = collect_input_parse_warnings(&[
            parse_diagnostic(2, "WithInStrict"),
            parse_diagnostic(3, "TS1102"),
            parse_diagnostic(5, "WithInStrict"),
            parse_diagnostic(8, "TS1102"),
        ]);

        assert_eq!(warnings.len(), 2, "warnings should coalesce: {warnings:#?}");
        assert!(warnings[0].message.contains("WithInStrict"));
        assert!(warnings[0].message.contains("2 occurrences"));
        assert!(warnings[0].message.contains("classic-script.js:2:1"));
        assert!(warnings[1].message.contains("TS1102"));
        assert!(warnings[1].message.contains("2 occurrences"));
        assert!(warnings[1].message.contains("classic-script.js:3:1"));
    }
}
