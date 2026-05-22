use anyhow::Result;
use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::visit::VisitMutWith;

use super::diagnostics::{
    collect_duplicate_declaration_warnings, collect_input_parse_warnings, collect_tdz_warnings,
    verify_output_parses,
};
use super::io::{parse_js_with_recovery, print_js};
use super::types::{DecompileOptions, DecompileOutput};
use crate::rules::{apply_rules, ImportDedup, RulePipelineOptions, UnImportRename};
use crate::sourcemap_rename::{apply_sourcemap_renames, parse_sourcemap};

pub fn decompile(source: &str, options: DecompileOptions) -> Result<DecompileOutput> {
    let span = tracing::info_span!("decompile", filename = %options.filename);
    let _enter = span.enter();

    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let parsed = {
            let span = tracing::info_span!("parse");
            let _enter = span.enter();
            parse_js_with_recovery(source, &options.filename, cm.clone())?
        };
        let mut module = parsed.module;

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        {
            let span = tracing::info_span!("resolver");
            let _enter = span.enter();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        }

        {
            let span = tracing::info_span!("rules");
            let _enter = span.enter();
            apply_rules(
                &mut module,
                unresolved_mark,
                RulePipelineOptions::default()
                    .with_dead_code_elimination(options.dead_code_elimination)
                    .with_rewrite_level(options.level),
            );
        }

        // Source-map-enhanced passes (only when sourcemap bytes are supplied).
        if let Some(bytes) = &options.sourcemap {
            let span = tracing::info_span!("sourcemap_renames");
            let _enter = span.enter();
            let sm = parse_sourcemap(bytes)?;
            module.visit_mut_with(&mut ImportDedup);
            apply_sourcemap_renames(&mut module, &sm, &cm, unresolved_mark);
            module.visit_mut_with(&mut UnImportRename);
        }

        let mut warnings = if options.diagnostics {
            let mut warnings = collect_input_parse_warnings(&parsed.recoverable_errors);
            warnings.extend(collect_tdz_warnings(&module, &options.filename));
            warnings.extend(collect_duplicate_declaration_warnings(
                &module,
                &options.filename,
            ));
            warnings
        } else {
            Vec::new()
        };

        {
            let span = tracing::info_span!("fixer");
            let _enter = span.enter();
            module.visit_mut_with(&mut fixer(None));
        }

        let code = {
            let span = tracing::info_span!("emit");
            let _enter = span.enter();
            print_js(&module, cm)?
        };

        if options.diagnostics {
            warnings.extend(verify_output_parses(&code, &options.filename));
        }

        Ok(DecompileOutput { code, warnings })
    })
}

#[cfg(test)]
mod tests {
    use super::super::types::UnpackWarningKind;
    use super::*;

    #[test]
    fn diagnostics_off_by_default() {
        let output = decompile("console.log(x);\nlet x = 1;", DecompileOptions::default())
            .expect("decompile should succeed");
        assert!(
            output.warnings.is_empty(),
            "default decompile should produce no diagnostic warnings"
        );
    }

    #[test]
    fn diagnostics_opt_in_reports_tdz() {
        let output = decompile(
            "console.log(x);\nlet x = 1;",
            DecompileOptions {
                diagnostics: true,
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::TdzViolation),
            "diagnostics should report TDZ violation"
        );
    }

    #[test]
    fn diagnostics_opt_in_reports_recovered_input_parse_errors() {
        let output = decompile(
            "label: label: break label;",
            DecompileOptions {
                diagnostics: true,
                filename: "duplicate-label.js".to_string(),
                ..Default::default()
            },
        )
        .expect("recovered input parse should still decompile");
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::InputParseRecovered),
            "diagnostics should report recovered input parse error: {:?}",
            output.warnings
        );
    }

    #[test]
    fn diagnostics_opt_in_reports_recovered_output_parse_errors() {
        let output = decompile(
            "label: label: break label;",
            DecompileOptions {
                diagnostics: true,
                filename: "duplicate-label.js".to_string(),
                ..Default::default()
            },
        )
        .expect("recovered output parse should still return emitted code");
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::OutputParseRecovered),
            "diagnostics should report recovered output parse error: {:?}",
            output.warnings
        );
        assert!(
            output.has_errors(),
            "recovered output parse errors should be error-severity diagnostics"
        );
    }

    #[test]
    fn diagnostics_opt_in_reports_duplicate_lexical_declarations() {
        let output = decompile(
            "let a = 1;\nlet a = 2;",
            DecompileOptions {
                diagnostics: true,
                filename: "duplicate.js".to_string(),
                ..Default::default()
            },
        )
        .expect("duplicate declarations should still return emitted code");
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::DuplicateDeclaration),
            "diagnostics should report duplicate declaration: {:?}",
            output.warnings
        );
        assert!(
            output.has_errors(),
            "duplicate declarations should be error-severity diagnostics"
        );
    }

    #[test]
    fn diagnostics_duplicate_declarations_inside_block() {
        let output = decompile(
            "{ const x = 1; const x = 2; }",
            DecompileOptions {
                diagnostics: true,
                filename: "block-dup.js".to_string(),
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::DuplicateDeclaration),
            "should detect duplicate declarations inside block: {:?}",
            output.warnings
        );
    }

    #[test]
    fn diagnostics_no_false_positive_across_block_scopes() {
        let output = decompile(
            "{ const x = 1; } { const x = 2; }",
            DecompileOptions {
                diagnostics: true,
                filename: "separate-scopes.js".to_string(),
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        assert!(
            !output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::DuplicateDeclaration),
            "separate block scopes should not report duplicate: {:?}",
            output.warnings
        );
    }

    #[test]
    fn diagnostics_no_false_positive_across_for_of_loops() {
        let output = decompile(
            "function f(items) { for (const x of items) { } for (const x of items) { } }",
            DecompileOptions {
                diagnostics: true,
                filename: "for-of-scopes.js".to_string(),
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        assert!(
            !output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::DuplicateDeclaration),
            "separate for-of loop scopes should not report duplicate: {:?}",
            output.warnings
        );
    }

    #[test]
    fn diagnostics_valid_output_no_parse_warning() {
        let output = decompile(
            "const x = 1;",
            DecompileOptions {
                diagnostics: true,
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        assert!(
            !output.warnings.iter().any(|w| matches!(
                w.kind,
                UnpackWarningKind::OutputParseRecovered | UnpackWarningKind::OutputParseFailed
            )),
            "valid output should not produce parse warning"
        );
    }
}
