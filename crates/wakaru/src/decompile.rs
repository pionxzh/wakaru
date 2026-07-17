use crate::error::{from_core_driver_error, Error, ErrorKind, Result};
use crate::options::DecompileOptions;
use crate::output::{
    DecompileOutput, Diagnostic, DiagnosticCode, DiagnosticSeverity, EntryStatus, ModuleOutput,
    ModuleStatus,
};
use crate::source::Source;

pub fn decompile(input: Source, options: DecompileOptions) -> Result<DecompileOutput> {
    let input = input.into_parts();
    let core_options = wakaru_core::DecompileOptions {
        filename: input.filename.clone(),
        sourcemap: input.source_map,
        dce_mode: options.rewrite().dce().into_core(),
        level: options.rewrite().level().into_core(),
        diagnostics: options.diagnostics(),
        emit_source_map: options.output_source_map(),
        ..Default::default()
    };

    match wakaru_core::driver::decompile_owned(input.code, core_options) {
        Ok(output) => {
            let diagnostics = output
                .warnings
                .into_iter()
                .map(|warning| diagnostic_from_core(warning, Some(0)))
                .collect();
            Ok(DecompileOutput {
                module: ModuleOutput {
                    filename: input.filename,
                    code: output.code,
                    source_map: output.source_map,
                    provenance: Vec::new(),
                    entry: EntryStatus::Unknown,
                    status: ModuleStatus::Decompiled,
                },
                diagnostics,
            })
        }
        Err(failure) => {
            let kind = from_core_driver_error(failure.kind);
            let error = failure.error;
            let message = error.to_string();
            if matches!(kind, ErrorKind::Parse | ErrorKind::SourceMap) {
                return Err(Error::new(kind, Some(input.filename), error));
            }

            Ok(DecompileOutput {
                module: ModuleOutput {
                    filename: input.filename.clone(),
                    code: failure.original_source,
                    source_map: None,
                    provenance: Vec::new(),
                    entry: EntryStatus::Unknown,
                    status: ModuleStatus::DecompileFailed,
                },
                diagnostics: vec![Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: DiagnosticCode::DecompileFailed,
                    message: format!("decompile failed, preserving original input: {message}"),
                    input: None,
                    module: Some(0),
                    span: None,
                }],
            })
        }
    }
}

pub(crate) fn diagnostic_from_core(
    warning: wakaru_core::UnpackWarning,
    module: Option<usize>,
) -> Diagnostic {
    let code = match warning.kind {
        wakaru_core::UnpackWarningKind::RawNormalizationFailed => {
            DiagnosticCode::RawNormalizationFailed
        }
        wakaru_core::UnpackWarningKind::FactCollectionParseFailed => {
            DiagnosticCode::FactCollectionFailed
        }
        wakaru_core::UnpackWarningKind::DecompileFailed => DiagnosticCode::DecompileFailed,
        wakaru_core::UnpackWarningKind::InputParseRecovered => DiagnosticCode::InputParseRecovered,
        wakaru_core::UnpackWarningKind::TdzViolation => DiagnosticCode::TdzViolation,
        wakaru_core::UnpackWarningKind::DuplicateDeclaration => {
            DiagnosticCode::DuplicateDeclaration
        }
        wakaru_core::UnpackWarningKind::ImportCycle => DiagnosticCode::ImportCycle,
        wakaru_core::UnpackWarningKind::OutputParseRecovered => {
            DiagnosticCode::OutputParseRecovered
        }
        wakaru_core::UnpackWarningKind::OutputParseFailed => DiagnosticCode::OutputParseFailed,
    };
    Diagnostic {
        severity: if warning.kind.is_error() {
            DiagnosticSeverity::Error
        } else {
            DiagnosticSeverity::Warning
        },
        code,
        message: warning.message,
        input: None,
        module,
        span: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompile_returns_owned_module_artifact() {
        let output = decompile(
            Source::new("named.js", "var value = 1;"),
            DecompileOptions::default(),
        )
        .expect("decompile should succeed");

        assert_eq!(output.module.filename, "named.js");
        assert!(output.module.code.contains("value = 1"));
        assert_eq!(output.module.entry, EntryStatus::Unknown);
        assert_eq!(output.module.status, ModuleStatus::Decompiled);
        assert!(output.module.provenance.is_empty());
    }

    #[test]
    fn unrecoverable_parse_is_a_typed_error() {
        let error = decompile(
            Source::new("broken.js", "function ("),
            DecompileOptions::default(),
        )
        .expect_err("invalid input should fail");

        assert_eq!(error.kind(), ErrorKind::Parse);
        assert_eq!(error.input_filename(), Some("broken.js"));
    }

    #[test]
    fn invalid_source_map_is_a_typed_error() {
        let error = decompile(
            Source::new("input.js", "const value = 1;")
                .with_source_map(b"not a source map".to_vec()),
            DecompileOptions::default(),
        )
        .expect_err("invalid source map should fail");

        assert_eq!(error.kind(), ErrorKind::SourceMap);
        assert_eq!(error.input_filename(), Some("input.js"));
    }

    #[test]
    fn helper_accepts_anyhow_for_internal_errors() {
        let error = Error::new(ErrorKind::Internal, None, anyhow::anyhow!("boom"));
        assert_eq!(error.to_string(), "boom");
    }
}
