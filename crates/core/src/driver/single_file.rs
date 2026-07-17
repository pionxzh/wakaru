use anyhow::Result;
use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;

use super::diagnostics::{
    collect_duplicate_declaration_warnings, collect_input_parse_warnings, collect_tdz_warnings,
    verify_output_parses,
};
use super::error::DriverErrorKind;
use super::io::{
    apply_fixer, build_output_sourcemap, parse_js_with_recovery_owned, print_js,
    print_js_with_srcmap,
};
use super::types::{DecompileOptions, DecompileOutput};
use crate::rules::{apply_rules, ImportDedup, RulePipelineOptions, UnImportRename};
use crate::sourcemap_rename::{apply_sourcemap_renames, parse_sourcemap};

pub fn decompile(source: &str, options: DecompileOptions) -> Result<DecompileOutput> {
    decompile_owned(source.to_string(), options).map_err(|failure| failure.error)
}

#[derive(Debug)]
pub struct OwnedDecompileFailure {
    pub kind: DriverErrorKind,
    pub error: anyhow::Error,
    pub original_source: String,
}

pub fn decompile_owned(
    source: String,
    options: DecompileOptions,
) -> std::result::Result<DecompileOutput, OwnedDecompileFailure> {
    let span = tracing::info_span!("decompile", filename = %options.filename);
    let _enter = span.enter();

    let cm: Lrc<SourceMap> = Default::default();
    let result = GLOBALS.set(&Default::default(), || {
        let parsed = {
            let span = tracing::info_span!("parse");
            let _enter = span.enter();
            parse_js_with_recovery_owned(source, &options.filename, cm.clone())
                .map_err(|error| (DriverErrorKind::Parse, error))?
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
                    .with_dce_mode(options.dce_mode)
                    .with_rewrite_level(options.level),
            );
        }

        // Source-map-enhanced passes (only when sourcemap bytes are supplied).
        if let Some(bytes) = &options.sourcemap {
            let span = tracing::info_span!("sourcemap_renames");
            let _enter = span.enter();
            let sm = parse_sourcemap(bytes).map_err(|error| (DriverErrorKind::SourceMap, error))?;
            module.visit_mut_with(&mut ImportDedup);
            apply_sourcemap_renames(&mut module, &sm, &cm, unresolved_mark);
            module.visit_mut_with(&mut UnImportRename::new(unresolved_mark));
        }

        if options.level >= crate::rules::RewriteLevel::Standard {
            crate::rules::strip_redundant_sentry_source_file(&mut module, &options.filename);
        }

        let mut warnings = collect_input_parse_warnings(&parsed.recoverable_errors);
        if options.diagnostics {
            warnings.extend(collect_tdz_warnings(&module, &options.filename));
            warnings.extend(collect_duplicate_declaration_warnings(
                &module,
                &options.filename,
            ));
        }

        {
            let span = tracing::info_span!("fixer");
            let _enter = span.enter();
            apply_fixer(&mut module).map_err(|error| (DriverErrorKind::Internal, error))?;
        }

        let (code, source_map) = {
            let span = tracing::info_span!("emit");
            let _enter = span.enter();
            if options.emit_source_map {
                let (code, srcmap_buf) = print_js_with_srcmap(&module, cm.clone())
                    .map_err(|error| (DriverErrorKind::Internal, error))?;
                let map_json = build_output_sourcemap(&srcmap_buf, &cm, &options.filename)
                    .map_err(|error| (DriverErrorKind::Internal, error))?;
                (code, Some(map_json))
            } else {
                (
                    print_js(&module, cm.clone())
                        .map_err(|error| (DriverErrorKind::Internal, error))?,
                    None,
                )
            }
        };

        if options.diagnostics {
            warnings.extend(verify_output_parses(&code, &options.filename));
        }

        Ok(DecompileOutput {
            code,
            warnings,
            source_map,
        })
    });

    result.map_err(|(kind, error)| {
        let original_source = cm
            .files()
            .first()
            .map(|file| file.src.to_string())
            .unwrap_or_default();
        OwnedDecompileFailure {
            kind,
            error,
            original_source,
        }
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
    fn owned_decompile_returns_original_source_on_failure() {
        let source = "function (".to_string();
        let failure = decompile_owned(source.clone(), DecompileOptions::default())
            .expect_err("invalid input should fail");
        assert_eq!(failure.original_source, source);
        assert_eq!(failure.kind, DriverErrorKind::Parse);
        assert!(failure.error.to_string().contains("failed to parse"));
    }

    #[test]
    fn owned_decompile_classifies_invalid_source_map_without_message_matching() {
        let failure = decompile_owned(
            "const value = 1;".to_string(),
            DecompileOptions {
                sourcemap: Some(b"not a source map".to_vec()),
                ..Default::default()
            },
        )
        .expect_err("invalid source map should fail");

        assert_eq!(failure.kind, DriverErrorKind::SourceMap);
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
    fn recovered_input_parse_errors_are_operational_diagnostics() {
        let output = decompile(
            "label: label: break label;",
            DecompileOptions {
                filename: "duplicate-label.js".to_string(),
                ..Default::default()
            },
        )
        .expect("recovered input parse should still decompile");
        assert!(output
            .warnings
            .iter()
            .any(|warning| warning.kind == UnpackWarningKind::InputParseRecovered));
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

    #[test]
    fn emit_source_map_off_by_default() {
        let output = decompile("const x = 1;", DecompileOptions::default())
            .expect("decompile should succeed");
        assert!(
            output.source_map.is_none(),
            "source map should not be generated by default"
        );
    }

    #[test]
    fn emit_source_map_produces_valid_v3_map() {
        let output = decompile(
            "var a = 1;\nvar b = a && a.x && a.x.y;",
            DecompileOptions {
                filename: "test-input.js".to_string(),
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        let map_json = output
            .source_map
            .expect("source map should be generated when emit_source_map is true");

        let sm = sourcemap::SourceMap::from_reader(map_json.as_bytes())
            .expect("source map JSON should parse as valid v3");
        assert_eq!(sm.get_file(), Some("test-input.js"));
        assert!(sm.get_source_count() > 0, "should have at least one source");
        assert!(sm.get_token_count() > 0, "should have mapping tokens");
    }

    #[test]
    fn emit_source_map_includes_source_content() {
        let input = "const greeting = 'hello';";
        let output = decompile(
            input,
            DecompileOptions {
                filename: "content-test.js".to_string(),
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        let map_json = output.source_map.expect("source map should be generated");
        let sm = sourcemap::SourceMap::from_reader(map_json.as_bytes())
            .expect("source map JSON should parse");
        assert_eq!(
            sm.get_source_contents(0),
            Some(input),
            "source map should embed the original input as sourcesContent"
        );
    }

    #[test]
    fn emit_source_map_tokens_map_back_to_input() {
        let input = "var x = 42;\nvar y = x + 1;";
        let output = decompile(
            input,
            DecompileOptions {
                filename: "mapping-test.js".to_string(),
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        let map_json = output.source_map.expect("source map should be generated");
        let sm = sourcemap::SourceMap::from_reader(map_json.as_bytes())
            .expect("source map JSON should parse");

        let has_line_0 = sm.tokens().any(|t| t.get_src_line() == 0);
        let has_line_1 = sm.tokens().any(|t| t.get_src_line() == 1);
        assert!(has_line_0, "should have tokens mapping to input line 0");
        assert!(has_line_1, "should have tokens mapping to input line 1");
    }
}
