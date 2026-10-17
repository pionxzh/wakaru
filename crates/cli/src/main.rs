use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use rayon::prelude::*;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;
#[cfg(test)]
use wakaru_core::{decompile, normalize, NormalizeOptions};
use wakaru_core::{
    format_trace_events, is_likely_vue_sfc_source, recover_vue_sfcs_from_js, trace_rules, DceMode,
    DecompileOptions, RewriteLevel, RuleTraceOptions, VueSfcRecoveryOptions,
};

mod bun_extract;
mod color;
mod discovery;
mod formatter;
mod json_output;
mod output;
mod vue;

use color::Styled;
use discovery::{collect_directory_js_inputs, collect_validate_inputs, DirectoryScanStats};
use formatter::{format_cli_output, format_cli_output_with_source_map, selected_formatter};
use json_output::{
    JsonChunkAsset, JsonChunkEnumeration, JsonChunkEnumerationOutput, JsonChunkUrl,
    JsonDecompileOutput, JsonModule, JsonModuleKind, JsonModuleStatus, JsonPublicPath,
    JsonRelativeImport, JsonUnpackOutput, JsonWarning,
};
use output::{canonicalize_output_dir, resolve_unpack_output_path, write_file, write_if_changed};
use vue::{
    ensure_vue_sidecar_does_not_overwrite_input, format_vue_sfc_artifact_summary,
    is_vue_output_path, recover_single_file_vue_after_unpack, recover_single_file_vue_sidecar,
    resolve_unpack_import_source, single_file_vue_metadata, single_file_vue_sidecar_path,
    vue_js_output_filename, vue_output_filename_for_component, vue_sfc_artifact_summary,
    vue_sfc_js_artifact_status,
};

// AST processing allocates and frees many small nodes across Rayon workers.
// Keep this allocator choice in the executable; library users own theirs.
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliRewriteLevel {
    Minimal,
    Standard,
    Aggressive,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum UnpackMode {
    /// Auto-detect bundle format, with heuristic fallback for scope-hoisted bundles.
    Auto,
    /// Structural detection only (webpack, browserify, esbuild). No heuristic fallback.
    Strict,
    /// Retain fine-grained scope-hoist boundaries for static inspection.
    /// The emitted module graph may not be safe to execute.
    Inspect,
}

impl From<CliRewriteLevel> for RewriteLevel {
    fn from(value: CliRewriteLevel) -> Self {
        match value {
            CliRewriteLevel::Minimal => RewriteLevel::Minimal,
            CliRewriteLevel::Standard => RewriteLevel::Standard,
            CliRewriteLevel::Aggressive => RewriteLevel::Aggressive,
        }
    }
}

#[derive(Debug, Clone, Parser)]
#[command(
    name = "wakaru",
    version,
    about = "Fast JavaScript decompiler and bundle splitter",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Input JavaScript/TypeScript file(s), or Bun single-file executables with
    /// --unpack. With --unpack, directories are scanned recursively for
    /// bundle/chunk files.
    ///
    /// Use `-` to read from stdin. If omitted and stdin is piped, stdin is read
    /// automatically.
    inputs: Vec<PathBuf>,

    /// Output file, or output directory when --unpack is set. Prints to stdout when omitted.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Unpack a bundle into readable module files.
    ///
    /// Requires --output, which is treated as the output directory.
    ///
    /// Modes:
    ///   --unpack / --unpack=auto    Auto-detect + heuristic fallback for scope-hoisted bundles
    ///   --unpack=strict             Structural detection only (no heuristic fallback)
    ///   --unpack=inspect            Fine-grained static inspection (may not execute)
    #[arg(short, long, value_enum, num_args = 0..=1, default_missing_value = "auto")]
    unpack: Option<UnpackMode>,

    /// With --unpack, write raw unpacker output before the decompiler rule pipeline.
    #[arg(long, requires = "unpack")]
    raw: bool,

    /// With --unpack, write a provenance.json in the output directory mapping
    /// each module file to the byte ranges in the original input it was
    /// extracted from.
    #[arg(long, requires = "unpack")]
    provenance: bool,

    /// Source map file (.map) for single-file identifier recovery and import deduplication.
    /// Not supported with --unpack.
    #[arg(
        short = 'm',
        long = "source-map",
        alias = "sourcemap",
        value_name = "MAP"
    )]
    sourcemap: Option<PathBuf>,

    /// Rewrite aggressiveness level.
    #[arg(long, default_value = "standard", value_enum)]
    level: CliRewriteLevel,

    /// Remove all dead code (full reachability sweep). By default, only
    /// transform-induced dead code is removed; pre-existing dead code in the
    /// input is preserved.
    #[arg(long)]
    dce: bool,

    /// Run post-transform diagnostic checks and print results to stderr.
    #[arg(long)]
    diagnostics: bool,

    /// Recover Vue 3 render functions into best-effort .vue single-file components.
    #[arg(long)]
    vue_sfc: bool,

    /// Run a final formatter pass on decompiled output.
    #[arg(long)]
    formatter: bool,

    /// Emit a source map (.map) alongside each decompiled JavaScript output
    /// file, mapping the output back to the input (with --unpack, into the
    /// bundle). Requires -o/--output. Vue SFC sidecars are not mapped.
    #[arg(long = "emit-source-map")]
    emit_source_map: bool,

    /// Output machine-readable JSON to stdout instead of human-readable
    /// summaries. Warnings and errors are included in the JSON object.
    #[arg(long)]
    json: bool,

    /// Write a Chrome trace profile to the given file (open with chrome://tracing).
    #[arg(long, value_name = "FILE")]
    profile: Option<PathBuf>,

    /// Include per-rule spans in --profile output.
    #[arg(long, requires = "profile")]
    profile_rules: bool,

    /// Overwrite existing output files or non-empty output directories.
    #[arg(long, global = true)]
    force: bool,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    /// Extract original source files embedded in a source map's sourcesContent.
    Extract(ExtractArgs),

    /// Inspect and extract Bun single-file executable containers.
    Bun(bun_extract::BunArgs),

    /// Internal debugging commands.
    #[command(hide = true)]
    Debug(DebugArgs),
}

#[derive(Debug, Clone, Args)]
struct ExtractArgs {
    /// Source map file containing sourcesContent.
    map: PathBuf,

    /// Output directory.
    #[arg(short, long, value_name = "DIR")]
    output: PathBuf,
}

#[derive(Debug, Clone, Args)]
struct DebugArgs {
    #[command(subcommand)]
    command: DebugCommand,
}

#[derive(Debug, Clone, Subcommand)]
enum DebugCommand {
    /// Trace the single-file rule pipeline and print per-rule before/after output.
    Trace(TraceArgs),

    /// Canonicalize source for structure-only comparison (parse + reprint, with
    /// optional scope-correct alpha-renaming of local bindings). Used by the
    /// reproduction matrices to compare mangled/minified output structurally.
    Normalize(NormalizeArgs),

    /// Validate a directory of emitted modules as one graph: dangling relative
    /// references, imports of names the provider doesn't export, duplicate
    /// exports, and writes to imported or `const` bindings. Normal unpack
    /// output only — raw output carries no module-graph contract. Exits
    /// nonzero when findings exist.
    Validate(ValidateArgs),

    /// Statically enumerate chunk references without unpacking modules.
    EnumerateChunks(EnumerateChunksArgs),

    /// Report how each CommonJS export name of one module would be stored
    /// (getter, mirror, or property) and why other models were rejected,
    /// analyzed on the module as it reaches UnEsm. Single-file inputs only.
    CjsExports(CjsExportsArgs),
}

#[derive(Debug, Clone, Args)]
struct CjsExportsArgs {
    /// Input JavaScript file. Use `-` or omit to read from stdin.
    input: Option<PathBuf>,

    /// Print the report as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Args)]
struct EnumerateChunksArgs {
    /// Input JavaScript file. Use `-` or omit to read from stdin.
    input: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
struct ValidateArgs {
    /// Directory containing emitted modules (normal unpack output).
    dir: PathBuf,

    /// Original bundle input (file or directory; repeatable). Free identifiers
    /// in the output that are not free anywhere in these inputs are reported.
    #[arg(long, value_name = "PATH")]
    input: Vec<PathBuf>,

    /// Print findings as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Args)]
struct NormalizeArgs {
    /// Input JavaScript/TypeScript file. Use `-` or omit to read from stdin.
    input: Option<PathBuf>,

    /// Alpha-rename every local binding to a deterministic canonical name
    /// (`$0`, `$1`, …), leaving free/global identifiers untouched. This makes
    /// mangled and original code normalize to identical source.
    #[arg(long)]
    rename: bool,

    /// Run the oxc formatter on the canonicalized output.
    #[arg(long)]
    format: bool,
}

#[derive(Debug, Clone, Args)]
struct TraceArgs {
    /// Input JavaScript/TypeScript file.
    input: PathBuf,

    /// Output trace file. Prints to stdout when omitted.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Source map file (.map) for identifier recovery and import deduplication.
    #[arg(
        short = 'm',
        long = "source-map",
        alias = "sourcemap",
        value_name = "MAP"
    )]
    sourcemap: Option<PathBuf>,

    /// Include rules that ran but did not change the rendered output.
    #[arg(long)]
    all: bool,

    /// First rule to run when tracing.
    #[arg(long = "from", value_name = "RULE")]
    from: Option<String>,

    /// Last rule to run when tracing.
    #[arg(long = "until", value_name = "RULE")]
    until: Option<String>,

    /// Rewrite aggressiveness level.
    #[arg(long, default_value = "standard", value_enum)]
    level: CliRewriteLevel,

    /// Remove all dead code (full reachability sweep). By default, only
    /// transform-induced dead code is removed, as in a normal decompile.
    #[arg(long)]
    dce: bool,
}

fn main() -> Result<()> {
    install_panic_hook();

    let cli = Cli::parse();
    let _profile_guard = init_profile(cli.profile.as_deref(), cli.profile_rules)?;

    match cli.command.clone() {
        Some(Command::Extract(args)) => run_extract(args, cli.force),
        Some(Command::Bun(args)) => bun_extract::run(args, cli.force),
        Some(Command::Debug(args)) => run_debug(args, cli.force),
        None => run_default(cli),
    }
}

fn run_default(cli: Cli) -> Result<()> {
    if cli.unpack.is_some() && cli.output.is_none() {
        bail!("--unpack requires -o/--output to choose an output directory");
    }
    if cli.vue_sfc && cli.raw {
        bail!("--vue-sfc cannot be combined with --raw");
    }
    if cli.unpack.is_some() && cli.sourcemap.is_some() {
        bail!(
            "--source-map is not supported with --unpack because extracted module coordinates differ from bundle coordinates; --emit-source-map remains available for output maps"
        );
    }
    if cli.unpack.is_some() {
        run_unpack(cli)
    } else {
        run_single(cli)
    }
}

fn run_unpack(cli: Cli) -> Result<()> {
    let unpack_mode = cli.unpack.expect("checked by run_default");
    let js_formatter = selected_formatter(cli.formatter);
    let styled = if cli.json {
        Styled::off()
    } else {
        Styled::for_stderr()
    };

    let dce_mode = dce_mode_for_flag(cli.dce);

    let out_dir = cli.output.expect("checked above");
    let check_existing_writes = ensure_output_dir(&out_dir, cli.force)?;
    let out_dir = canonicalize_output_dir(&out_dir)?;

    let start = Instant::now();
    let execution = run_public_unpack(
        &cli.inputs,
        cli.raw,
        unpack_mode,
        dce_mode,
        cli.level.into(),
        cli.diagnostics,
        cli.emit_source_map,
    )?;
    let scan_stats = execution.scan_stats;
    let single_input_name = execution.single_input_name;
    let output = execution.output;
    for intake in &execution.gtm {
        if !cli.json {
            eprintln!(
                "gtm: {} container version {} -> {} script(s), {} external",
                intake.input,
                intake.container.version.as_deref().unwrap_or("unknown"),
                intake.container.scripts.len(),
                intake.container.external_scripts.len()
            );
        }
    }
    let elapsed = start.elapsed();

    if output.safety == wakaru::OutputSafety::InspectionOnly {
        eprintln!(
            "{}: --unpack=inspect output may not preserve runtime initialization order",
            styled.warning("warning")
        );
    }
    if !cli.json {
        print_warnings(&output.warnings, &styled);
    }
    let error_modules: Vec<&str> = output
        .warnings
        .iter()
        .filter(|w| w.is_error)
        .map(|w| w.filename.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let provenance = output.provenance;
    let module_sources = cli
        .vue_sfc
        .then(|| output.modules.iter().cloned().collect::<HashMap<_, _>>());
    let mut source_maps: HashMap<String, String> = output.source_maps.into_iter().collect();
    let modules: Vec<(String, String, Option<String>)> = output
        .modules
        .into_iter()
        .map(|(filename, code)| {
            let source_map = source_maps.remove(&filename);
            (filename, code, source_map)
        })
        .collect();
    let total_modules = modules.len();
    let artifacts: Vec<CliOutputArtifact> = modules
        .into_par_iter()
        .flat_map(|(filename, code, source_map)| {
            let mut artifacts = Vec::new();
            let recovered_vue_sfcs = if cli.vue_sfc {
                let module_sources = module_sources
                    .as_ref()
                    .expect("vue sfc module source map is initialized");
                recover_vue_sfcs_from_js(
                    &code,
                    VueSfcRecoveryOptions::default().with_import_resolver(|specifier| {
                        resolve_unpack_import_source(module_sources, &filename, specifier)
                    }),
                )
                .ok()
                .unwrap_or_default()
            } else {
                Vec::new()
            };
            let recovered_vue_sfc = !recovered_vue_sfcs.is_empty();
            let likely_vue_sfc = cli.vue_sfc
                && (recovered_vue_sfc || is_likely_vue_sfc_source(&code).unwrap_or(false));
            let (formatted, source_map) =
                format_cli_output_with_source_map(code, source_map, &filename, js_formatter);
            artifacts.push(CliOutputArtifact {
                filename: if cli.vue_sfc {
                    vue_js_output_filename(&filename)
                } else {
                    filename.clone()
                },
                code: formatted,
                kind: JsonModuleKind::JavaScript,
                status: if cli.vue_sfc {
                    vue_sfc_js_artifact_status(recovered_vue_sfc, likely_vue_sfc)
                } else {
                    JsonModuleStatus::Decompiled
                },
                source_filename: (cli.vue_sfc && recovered_vue_sfc).then(|| filename.clone()),
                source_map_filename: Some(filename.clone()),
                source_map,
            });

            let multiple_vue_sfcs = recovered_vue_sfcs.len() > 1;
            for recovered in recovered_vue_sfcs {
                artifacts.push(CliOutputArtifact {
                    filename: vue_output_filename_for_component(
                        &filename,
                        recovered.name.as_deref(),
                        multiple_vue_sfcs,
                    ),
                    code: recovered.sfc.print(),
                    kind: JsonModuleKind::VueSfc,
                    status: JsonModuleStatus::RecoveredVueSfc,
                    source_filename: Some(filename.clone()),
                    source_map_filename: None,
                    source_map: None,
                });
            }
            artifacts
        })
        .collect();

    let resolved: Vec<(PathBuf, &str)> = {
        let span = tracing::info_span!("cli_resolve_output_paths");
        let _enter = span.enter();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        artifacts
            .iter()
            .map(|artifact| {
                let out_path = resolve_unpack_output_path(&out_dir, &artifact.filename, &mut seen)?;
                Ok((out_path, artifact.code.as_str()))
            })
            .collect::<Result<_>>()?
    };

    {
        let span = tracing::info_span!("cli_write_output_files", count = resolved.len());
        let _enter = span.enter();
        if check_existing_writes {
            resolved
                .par_iter()
                .try_for_each(|(path, code)| write_if_changed(path, code))?;
        } else {
            resolved
                .par_iter()
                .try_for_each(|(path, code)| write_file(path, code))?;
        }
    }

    // Each container's index sits next to its pieces. It is not a module, so it is not in
    // `--json` output.
    let mut index_seen = std::collections::HashSet::new();
    for intake in &execution.gtm {
        let filename = format!("{}index.md", intake.prefix);
        let path = resolve_unpack_output_path(&out_dir, &filename, &mut index_seen)?;
        let index = gtm_index_markdown(&intake.container);
        if check_existing_writes {
            write_if_changed(&path, &index)?;
        } else {
            write_file(&path, &index)?;
        }
    }

    if artifacts
        .iter()
        .any(|artifact| artifact.source_map.is_some())
    {
        let span = tracing::info_span!("cli_write_source_maps");
        let _enter = span.enter();
        artifacts.par_iter().zip(resolved.par_iter()).try_for_each(
            |(artifact, (out_path, _))| match artifact.source_map.as_deref() {
                Some(map_json) => {
                    write_output_source_map(&append_map_extension(out_path), map_json)
                }
                None => Ok(()),
            },
        )?;
    }

    if cli.provenance {
        // Map each original module to the final JavaScript artifact path.
        // Recovered Vue SFC sidecars interleave with JS artifacts, so this
        // must use artifact metadata rather than zipping modules directly.
        let final_names = provenance_final_names(&artifacts, &resolved, &out_dir);
        let json = render_provenance_json(
            &provenance,
            &final_names,
            single_input_name.as_deref().unwrap_or(""),
            &output.detected_formats,
        );
        let provenance_path = out_dir.join("provenance.json");
        fs::write(&provenance_path, json)
            .with_context(|| format!("failed to write {}", provenance_path.display()))?;
    }

    if cli.json {
        let json = json_unpack_output_for_artifacts(
            &output.detected_formats,
            output.safety,
            &artifacts,
            &output.warnings,
            total_modules,
            error_modules.len(),
            elapsed,
        );
        println!(
            "{}",
            serde_json::to_string(&json).expect("JSON serialization")
        );
    } else if io::stderr().is_terminal() {
        if let Some(stats) = scan_stats {
            eprintln!(
                "scanned: {} file(s), detected: {} bundle/chunk file(s), skipped: {} file(s)",
                stats.scanned, stats.detected, stats.skipped
            );
        }
        if !output.detected_formats.is_empty() {
            let names: Vec<&str> = output.detected_formats.iter().map(|f| f.as_str()).collect();
            eprintln!("detected: {}", names.join(", "));
        }
        if let Some(summary) = vue_sfc_artifact_summary(&artifacts) {
            eprintln!("{}", format_vue_sfc_artifact_summary(summary));
        }
        let fail_info = if error_modules.is_empty() {
            String::new()
        } else {
            format!(" ({} failed)", error_modules.len())
        };
        eprintln!(
            "total: {} module(s){fail_info} in {}",
            styled.bold(&total_modules.to_string()),
            format_elapsed(elapsed),
        );
    }

    if !error_modules.is_empty() {
        bail!(
            "errors in {} module(s): {}",
            error_modules.len(),
            error_modules.join(", ")
        );
    }

    Ok(())
}

fn run_single(cli: Cli) -> Result<()> {
    let styled = if cli.json {
        Styled::off()
    } else {
        Styled::for_stderr()
    };

    if cli.inputs.len() > 1 {
        bail!("multiple input files require --unpack");
    }
    if let Some(input) = cli.inputs.first() {
        if input.is_dir() {
            bail!("cannot decompile a directory. Pass a JavaScript file or use --unpack");
        }
    }
    let (input, filename) = read_input(cli.inputs.first())?;
    let input_path_for_collision = cli
        .inputs
        .first()
        .filter(|path| *path != &PathBuf::from("-"))
        .cloned();
    let output_filename = filename.clone();
    let sourcemap_bytes = read_sourcemap(cli.sourcemap.as_ref())?;
    let dce_mode = dce_mode_for_flag(cli.dce);
    let output_path = cli.output.clone();
    let vue_file_output = cli.vue_sfc
        && output_path
            .as_ref()
            .is_some_and(|path| is_vue_output_path(path));
    let js_primary_vue_output = cli.vue_sfc
        && output_path
            .as_ref()
            .is_some_and(|path| !is_vue_output_path(path));
    let start = Instant::now();
    let vue_unpack_source = cli.vue_sfc.then(|| input.clone());
    let mut source = wakaru::Source::new(filename, input);
    if let Some(sourcemap) = sourcemap_bytes {
        source = source.with_source_map(sourcemap);
    }
    let rewrite = wakaru::RewriteOptions::default()
        .with_level(public_rewrite_level(cli.level.into()))
        .with_dce(public_dce_mode(dce_mode));
    let public_output = wakaru::decompile(
        source,
        wakaru::DecompileOptions::default()
            .with_rewrite(rewrite)
            .with_diagnostics(cli.diagnostics)
            .with_output_source_map(cli.emit_source_map),
    )?;
    let mut output = adapt_public_decompile_output(public_output);
    let recovered = cli
        .vue_sfc
        .then(|| recover_single_file_vue_sidecar(&output.code, &output_filename))
        .flatten()
        .or_else(|| {
            vue_unpack_source.as_deref().and_then(|source| {
                recover_single_file_vue_after_unpack(
                    source,
                    &output_filename,
                    rewrite,
                    cli.diagnostics,
                )
            })
        });
    let recovered_vue_sfc = recovered.is_some();
    let vue_sidecar = js_primary_vue_output.then_some(recovered.clone()).flatten();
    if vue_file_output {
        if let Some(recovered) = recovered {
            output.code = recovered;
            output.source_map = None;
        }
    }
    let vue_sidecar_path = output_path
        .as_ref()
        .filter(|_| js_primary_vue_output)
        .and_then(|path| {
            vue_sidecar
                .as_ref()
                .map(|_| single_file_vue_sidecar_path(&output_filename, path))
        });
    if let Some(ref sidecar_path) = vue_sidecar_path {
        ensure_vue_sidecar_does_not_overwrite_input(
            sidecar_path,
            input_path_for_collision.as_deref(),
        )?;
    }
    let elapsed = start.elapsed();

    if !cli.json {
        print_warnings(&output.warnings, &styled);
    }
    let has_errors = output.has_errors();
    if vue_file_output && !recovered_vue_sfc {
        bail!("--vue-sfc did not recover a Vue SFC; cannot write Vue-only output");
    }
    let vue_metadata = single_file_vue_metadata(
        cli.vue_sfc,
        recovered_vue_sfc,
        js_primary_vue_output,
        &output.code,
        &output_filename,
        vue_sidecar_path.as_deref(),
    );
    let formatter =
        selected_formatter(cli.formatter && (!recovered_vue_sfc || js_primary_vue_output));
    let (code, source_map) = format_cli_output_with_source_map(
        output.code,
        output.source_map.take(),
        &output_filename,
        formatter,
    );

    if cli.json {
        let json_code = if output_path.is_none() {
            Some(code.clone())
        } else {
            None
        };
        if let Some(ref path) = output_path {
            ensure_output_file(path, cli.force)?;
            if let Some(ref sidecar_path) = vue_sidecar_path {
                ensure_output_file(sidecar_path, cli.force)?;
            }
            fs::write(path, &code)
                .with_context(|| format!("failed to write {}", path.display()))?;
            if let Some(ref map_json) = source_map {
                write_output_source_map(&append_map_extension(path), map_json)?;
            }
            if let (Some(sidecar_path), Some(sidecar_code)) =
                (vue_sidecar_path.as_ref(), vue_sidecar.as_ref())
            {
                fs::write(sidecar_path, sidecar_code)
                    .with_context(|| format!("failed to write {}", sidecar_path.display()))?;
            }
        }
        let json = JsonDecompileOutput {
            code: json_code,
            source_map: source_map.clone(),
            kind: vue_metadata.as_ref().map(|metadata| metadata.kind),
            status: vue_metadata.as_ref().map(|metadata| metadata.status),
            source_filename: vue_metadata
                .as_ref()
                .and_then(|metadata| metadata.source_filename.clone()),
            vue_sidecar_filename: vue_metadata
                .as_ref()
                .and_then(|metadata| metadata.vue_sidecar_filename.clone()),
            warnings: output.warnings.iter().map(CliWarning::to_json).collect(),
            elapsed_ms: elapsed.as_millis() as u64,
        };
        println!(
            "{}",
            serde_json::to_string(&json).expect("JSON serialization")
        );
    } else {
        match output_path {
            Some(path) => {
                ensure_output_file(&path, cli.force)?;
                if let Some(ref sidecar_path) = vue_sidecar_path {
                    ensure_output_file(sidecar_path, cli.force)?;
                }
                fs::write(&path, &code)
                    .with_context(|| format!("failed to write {}", path.display()))?;
                if let Some(ref map_json) = source_map {
                    write_output_source_map(&append_map_extension(&path), map_json)?;
                }
                if let (Some(sidecar_path), Some(sidecar_code)) =
                    (vue_sidecar_path.as_ref(), vue_sidecar.as_ref())
                {
                    fs::write(sidecar_path, sidecar_code)
                        .with_context(|| format!("failed to write {}", sidecar_path.display()))?;
                }
            }
            None => {
                print!("{code}");
            }
        }
    }

    if has_errors {
        let failing: Vec<&str> = output
            .warnings
            .iter()
            .filter(|w| w.is_error)
            .map(|w| w.filename.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        bail!(
            "errors in {} module(s): {}",
            failing.len(),
            failing.join(", ")
        );
    }

    Ok(())
}

fn print_warnings(warnings: &[CliWarning], styled: &Styled) {
    for warning in warnings {
        let label = if warning.is_error {
            styled.error("error")
        } else {
            styled.warning("warning")
        };
        eprintln!("{label}: {}: {}", warning.filename, warning.message);
    }
}

fn format_elapsed(d: Duration) -> String {
    if d.as_secs() >= 1 {
        format!("{:.2}s", d.as_secs_f64())
    } else {
        format!("{}ms", d.as_millis())
    }
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        let version = env!("CARGO_PKG_VERSION");
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let body = format!("**Version:** {version}%0A**OS:** {os} {arch}%0A%0A```%0A<paste panic output here>%0A```");
        eprintln!();
        eprintln!("wakaru {version} ({os} {arch}) encountered an internal error.");
        eprintln!("This is a bug. Please report it at:");
        eprintln!(
            "  https://github.com/pionxzh/wakaru/issues/new?labels=pending+triage&title=Internal+error+in+wakaru+{version}&body={body}"
        );
    }));
}

fn run_extract(args: ExtractArgs, force: bool) -> Result<()> {
    let map_bytes = fs::read(&args.map)
        .with_context(|| format!("failed to read source map {}", args.map.display()))?;
    ensure_output_dir(&args.output, force)?;

    let entries = wakaru::sourcemap::embedded_sources(&map_bytes)?;
    let mut written = 0;
    for entry in &entries {
        let out_path = args.output.join(&entry.path);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(&out_path, &entry.content)
            .with_context(|| format!("failed to write {}", out_path.display()))?;
        written += 1;
    }

    if io::stderr().is_terminal() {
        eprintln!(
            "extracted {written} source file(s) to {}",
            args.output.display()
        );
    }
    Ok(())
}

fn run_debug(args: DebugArgs, force: bool) -> Result<()> {
    match args.command {
        DebugCommand::Trace(args) => run_trace(args, force),
        DebugCommand::Normalize(args) => run_normalize(args),
        DebugCommand::Validate(args) => run_validate(args),
        DebugCommand::EnumerateChunks(args) => run_enumerate_chunks(args),
        DebugCommand::CjsExports(args) => run_cjs_exports(args),
    }
}

fn run_cjs_exports(args: CjsExportsArgs) -> Result<()> {
    let (source, filename) = read_input(args.input.as_ref())?;
    let report = wakaru_core::explain_commonjs_exports(
        &source,
        wakaru_core::DecompileOptions {
            filename: filename.clone(),
            ..Default::default()
        },
    )
    .map_err(|error| error.into_inner())?;
    if args.json {
        let payload = serde_json::json!({
            "input": filename,
            "uses_exports": report.uses_exports,
            "gate": report.gate,
            "exports": report.exports.iter().map(|export| serde_json::json!({
                "name": export.name,
                "storage": export.storage,
                "binding": export.binding,
                "rejected": export.rejected,
                "writes": export.writes,
                "sentinels": export.sentinels,
                "other_writes": export.other_writes,
                "reads": export.reads,
                "calls": export.calls,
                "getters": export.getters,
                "deferred": export.deferred,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        print!("{}", cjs_exports_text(&report));
    }
    Ok(())
}

fn cjs_exports_text(report: &wakaru_core::CommonJsExportReport) -> String {
    if !report.uses_exports {
        return "no CommonJS exports\n".to_string();
    }
    if let Some(gate) = &report.gate {
        return format!("module gate failed: {gate}\n");
    }
    let mut out = String::new();
    for export in &report.exports {
        let binding = export
            .binding
            .as_ref()
            .map(|binding| format!(" `{binding}`"))
            .unwrap_or_default();
        out.push_str(&format!(
            "{}: {}{binding} (writes {}, sentinels {}, other writes {}, reads {}, calls {}, getters {}, deferred {})\n",
            export.name,
            export.storage,
            export.writes,
            export.sentinels,
            export.other_writes,
            export.reads,
            export.calls,
            export.getters,
            export.deferred,
        ));
        for rejected in &export.rejected {
            out.push_str(&format!("  - {rejected}\n"));
        }
    }
    out
}

fn run_enumerate_chunks(args: EnumerateChunksArgs) -> Result<()> {
    let (source, filename) = read_input(args.input.as_ref())?;
    let output = enumerate_chunks_json(&source, &filename)?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn enumerate_chunks_json(source: &str, filename: &str) -> Result<JsonChunkEnumerationOutput> {
    let report = wakaru_core::unpacker::enumerate_chunks(source, filename)?;
    Ok(JsonChunkEnumerationOutput {
        input: filename.to_string(),
        detected_format: report
            .detected_format
            .map(|format| format.as_str().to_string()),
        enumeration: report.enumeration.as_ref().map(json_chunk_enumeration),
    })
}

fn run_validate(args: ValidateArgs) -> Result<()> {
    let files = collect_validate_inputs(&args.dir)?;
    if files.is_empty() {
        anyhow::bail!("no JavaScript files found under {}", args.dir.display());
    }
    let mut modules = Vec::with_capacity(files.len());
    for path in &files {
        let relative = path
            .strip_prefix(&args.dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        modules.push((relative, source));
    }

    let mut inputs = Vec::new();
    for input in &args.input {
        let files = if input.is_dir() {
            collect_directory_js_inputs(input)?
        } else {
            vec![input.clone()]
        };
        for path in files {
            let source = fs::read_to_string(&path)
                .with_context(|| format!("failed to read input {}", path.display()))?;
            inputs.push((path.to_string_lossy().replace('\\', "/"), source));
        }
    }
    if !args.input.is_empty() && inputs.is_empty() {
        anyhow::bail!("no JavaScript files found under the --input paths");
    }

    let findings = wakaru_core::validate_output_modules_with_inputs(&modules, &inputs);

    if args.json {
        let payload = serde_json::json!({
            "modules": modules.len(),
            "inputs": inputs.len(),
            "findings": findings
                .iter()
                .map(validate_finding_json)
                .collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        for finding in &findings {
            println!("{}", format_validate_finding(finding));
        }
        if io::stderr().is_terminal() {
            eprintln!(
                "{} finding(s) across {} module(s)",
                findings.len(),
                modules.len()
            );
        }
    }

    if !findings.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

fn validate_finding_json(finding: &wakaru_core::OutputFinding) -> serde_json::Value {
    serde_json::json!({
        "filename": finding.filename,
        "line": finding.line,
        "column": finding.column,
        "kind": finding.kind.as_str(),
        "message": finding.message,
    })
}

fn format_validate_finding(finding: &wakaru_core::OutputFinding) -> String {
    format!(
        "{}:{}:{}: {}: {}",
        finding.filename,
        finding.line,
        finding.column,
        finding.kind.as_str(),
        finding.message
    )
}

fn run_normalize(args: NormalizeArgs) -> Result<()> {
    let (source, filename) = read_input(args.input.as_ref())?;
    let options = wakaru::debug::NormalizeOptions::default().with_rename_bindings(args.rename);
    let canonical = wakaru::debug::normalize(wakaru::Source::new(&filename, source), options)?;
    let output = if args.format {
        format_cli_output(canonical, &filename, selected_formatter(true))
    } else {
        canonical
    };
    print!("{output}");
    Ok(())
}

/// `--dce` selects the full sweep; without it, only dead code the transforms
/// created is removed.
fn dce_mode_for_flag(dce: bool) -> DceMode {
    if dce {
        DceMode::Full
    } else {
        DceMode::TransformOnly
    }
}

/// Trace runs the same DCE mode as a normal decompile, so its last rule's
/// output matches the CLI's.
fn trace_decompile_options(args: &TraceArgs, sourcemap: Option<Vec<u8>>) -> DecompileOptions {
    DecompileOptions {
        filename: args.input.to_string_lossy().to_string(),
        sourcemap,
        level: args.level.into(),
        dce_mode: dce_mode_for_flag(args.dce),
        ..Default::default()
    }
}

fn run_trace(args: TraceArgs, force: bool) -> Result<()> {
    let input = fs::read_to_string(&args.input)
        .with_context(|| format!("failed to read {}", args.input.display()))?;
    let sourcemap_bytes = read_sourcemap(args.sourcemap.as_ref())?;
    let options = trace_decompile_options(&args, sourcemap_bytes);
    let events = trace_rules(
        &input,
        options,
        RuleTraceOptions {
            start_from: args.from,
            stop_after: args.until,
            only_changed: !args.all,
        },
    )?;
    let output = trace_output_text(&events, args.all);

    match args.output {
        Some(path) => {
            ensure_output_file(&path, force)?;
            fs::write(&path, output)
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
        None => {
            print!("{output}");
        }
    }

    Ok(())
}

/// A traced range in which no rule changed the rendered output yields no
/// events; say so instead of emitting an empty trace.
fn trace_output_text(events: &[wakaru_core::RuleTraceEvent], include_unchanged: bool) -> String {
    if !events.is_empty() {
        return format_trace_events(events);
    }
    if include_unchanged {
        "=== no rule ran in the traced range ===\n".to_string()
    } else {
        "=== no rule changed the rendered output in the traced range (pass --all to list unchanged rules) ===\n"
            .to_string()
    }
}

fn read_sourcemap(path: Option<&PathBuf>) -> Result<Option<Vec<u8>>> {
    match path {
        Some(p) => {
            let bytes = fs::read(p)
                .with_context(|| format!("failed to read source map {}", p.display()))?;
            Ok(Some(bytes))
        }
        None => Ok(None),
    }
}

fn read_input(input: Option<&PathBuf>) -> Result<(String, String)> {
    match input {
        Some(path) if path == &PathBuf::from("-") => {
            let mut code = String::new();
            io::stdin()
                .read_to_string(&mut code)
                .context("failed to read stdin")?;
            Ok((code, "<stdin>".to_string()))
        }
        Some(path) => {
            let code = fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            Ok((code, path.to_string_lossy().to_string()))
        }
        None if !io::stdin().is_terminal() => {
            let mut code = String::new();
            io::stdin()
                .read_to_string(&mut code)
                .context("failed to read stdin")?;
            Ok((code, "<stdin>".to_string()))
        }
        None => {
            bail!("no input specified; pass a file path or pipe code on stdin")
        }
    }
}

struct CliOutputArtifact {
    filename: String,
    code: String,
    kind: JsonModuleKind,
    status: JsonModuleStatus,
    source_filename: Option<String>,
    source_map_filename: Option<String>,
    /// Output source map for `code`, already carried across formatting.
    source_map: Option<String>,
}

fn json_module_for_artifact(artifact: &CliOutputArtifact) -> JsonModule {
    JsonModule {
        filename: artifact.filename.clone(),
        kind: artifact.kind,
        status: artifact.status,
        source_filename: artifact.source_filename.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn json_unpack_output_for_artifacts(
    detected_formats: &[CliBundleFormat],
    safety: wakaru::OutputSafety,
    artifacts: &[CliOutputArtifact],
    warnings: &[CliWarning],
    total_modules: usize,
    failed: usize,
    elapsed: Duration,
) -> JsonUnpackOutput {
    JsonUnpackOutput {
        detected_formats: detected_formats
            .iter()
            .map(|format| format.as_str().to_string())
            .collect(),
        safety: match safety {
            wakaru::OutputSafety::Normal => "normal",
            wakaru::OutputSafety::InspectionOnly => "inspection-only",
            _ => "unknown",
        }
        .to_string(),
        modules: artifacts.iter().map(json_module_for_artifact).collect(),
        warnings: warnings.iter().map(CliWarning::to_json).collect(),
        total: total_modules,
        failed,
        elapsed_ms: elapsed.as_millis() as u64,
    }
}

fn json_chunk_enumeration(
    enumeration: &wakaru_core::unpacker::chunk_enumeration::ChunkEnumeration,
) -> JsonChunkEnumeration {
    use wakaru_core::unpacker::chunk_enumeration::{
        ChunkAssetKind, ChunkEnumerationStatus, ChunkIdSource, PublicPathFact, RelativeImportKind,
    };

    JsonChunkEnumeration {
        public_path: match &enumeration.public_path {
            PublicPathFact::Static(value) => JsonPublicPath {
                status: "static".to_string(),
                value: Some(value.clone()),
            },
            PublicPathFact::ScriptRelative(suffix) => JsonPublicPath {
                status: "script_relative".to_string(),
                value: Some(suffix.clone()),
            },
            PublicPathFact::RuntimeComputed => JsonPublicPath {
                status: "runtime_computed".to_string(),
                value: None,
            },
            PublicPathFact::NotFound => JsonPublicPath {
                status: "not_found".to_string(),
                value: None,
            },
        },
        assets: enumeration
            .assets
            .iter()
            .map(|asset| JsonChunkAsset {
                kind: match asset.kind {
                    ChunkAssetKind::Js => "js",
                    ChunkAssetKind::Css => "css",
                }
                .to_string(),
                status: match asset.status {
                    ChunkEnumerationStatus::Enumerated => "enumerated",
                    ChunkEnumerationStatus::NoStaticChunkIds => "no_static_chunk_ids",
                    ChunkEnumerationStatus::DynamicTemplate => "dynamic_template",
                }
                .to_string(),
                template: asset.template.clone(),
                urls: asset
                    .urls
                    .iter()
                    .map(|url| JsonChunkUrl {
                        chunk_id: url.chunk_id.clone(),
                        url: url.url.clone(),
                        source: match url.source {
                            ChunkIdSource::FilenameMap => "filename_map",
                            ChunkIdSource::EnsureCall => "ensure_call",
                        }
                        .to_string(),
                    })
                    .collect(),
            })
            .collect(),
        relative_imports: enumeration
            .relative_imports
            .iter()
            .map(|import| JsonRelativeImport {
                specifier: import.specifier.clone(),
                kind: match import.kind {
                    RelativeImportKind::Import => "import",
                    RelativeImportKind::ExportFrom => "export_from",
                    RelativeImportKind::DynamicImport => "dynamic_import",
                }
                .to_string(),
            })
            .collect(),
    }
}

struct PublicUnpackExecution {
    output: CliUnpackOutput,
    scan_stats: Option<DirectoryScanStats>,
    single_input_name: Option<String>,
    gtm: Vec<GtmIntake>,
}

struct CliDecompileOutput {
    code: String,
    source_map: Option<String>,
    warnings: Vec<CliWarning>,
}

impl CliDecompileOutput {
    fn has_errors(&self) -> bool {
        self.warnings.iter().any(|warning| warning.is_error)
    }
}

#[derive(Debug, Clone)]
struct CliWarning {
    filename: String,
    kind: String,
    is_error: bool,
    message: String,
}

impl CliWarning {
    fn from_public(diagnostic: wakaru::Diagnostic, filename: String) -> Self {
        Self::new(
            filename,
            diagnostic.code,
            diagnostic.severity,
            diagnostic.message,
        )
    }

    fn new(
        filename: String,
        code: wakaru::DiagnosticCode,
        severity: wakaru::DiagnosticSeverity,
        message: String,
    ) -> Self {
        Self {
            filename,
            kind: code.as_str().to_string(),
            is_error: severity == wakaru::DiagnosticSeverity::Error,
            message,
        }
    }

    fn to_json(&self) -> JsonWarning {
        JsonWarning::new(
            self.filename.clone(),
            self.kind.clone(),
            self.is_error,
            self.message.clone(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliBundleFormat {
    Structural(wakaru::BundleFormat),
    ScopeHoisted,
}

impl CliBundleFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Structural(format) => format.as_str(),
            Self::ScopeHoisted => "scope-hoisted",
        }
    }

    fn is_scope_hoisted(self) -> bool {
        self == Self::ScopeHoisted
    }
}

#[derive(Debug, Clone)]
struct CliModuleProvenance {
    filename: String,
    input: String,
    ranges: Vec<(u32, u32)>,
    inspection_context_ranges: Vec<(u32, u32)>,
}

struct CliUnpackOutput {
    modules: Vec<(String, String)>,
    provenance: Vec<CliModuleProvenance>,
    warnings: Vec<CliWarning>,
    detected_formats: Vec<CliBundleFormat>,
    source_maps: Vec<(String, String)>,
    safety: wakaru::OutputSafety,
}

fn adapt_public_decompile_output(output: wakaru::DecompileOutput) -> CliDecompileOutput {
    let filename = output.module.filename.clone();
    CliDecompileOutput {
        code: output.module.code,
        source_map: output.module.source_map,
        warnings: output
            .diagnostics
            .into_iter()
            .map(|diagnostic| CliWarning::from_public(diagnostic, filename.clone()))
            .collect(),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_public_unpack(
    paths: &[PathBuf],
    raw: bool,
    unpack_mode: UnpackMode,
    dce_mode: DceMode,
    level: RewriteLevel,
    diagnostics: bool,
    emit_source_map: bool,
) -> Result<PublicUnpackExecution> {
    let saw_directory = paths.iter().any(|path| path.is_dir());
    let rewrite = wakaru::RewriteOptions::default()
        .with_level(public_rewrite_level(level))
        .with_dce(public_dce_mode(dce_mode));
    let options = wakaru::UnpackOptions::default()
        .with_modules(if raw {
            wakaru::ModuleMode::Raw
        } else {
            wakaru::ModuleMode::Decompile(rewrite)
        })
        .with_mode(public_unpack_mode(unpack_mode))
        .with_unmatched(wakaru::UnmatchedInput::Process)
        .with_diagnostics(diagnostics)
        .with_output_source_maps(emit_source_map);
    let mut job = wakaru::UnpackJob::new(options)?;
    let mut gtm = Vec::new();
    let mut stats = DirectoryScanStats::default();
    let mut pushed_inputs = 0usize;
    // inputs the job itself received; a run of containers only has none
    let mut job_inputs = 0usize;

    {
        let span = tracing::info_span!("cli_unpack_intake");
        let _enter = span.enter();
        // Pieces of several containers need distinct names; one input keeps the short layout.
        let prefix_gtm_pieces = paths.len() > 1;
        if paths.is_empty() {
            let (code, filename) = read_input(None)?;
            job_inputs += usize::from(push_explicit_source(
                &mut job,
                wakaru::Source::new(filename, code),
                prefix_gtm_pieces,
                &mut gtm,
            )?);
            pushed_inputs += 1;
        } else {
            for path in paths {
                if path == &PathBuf::from("-") || !path.is_dir() {
                    if path == &PathBuf::from("-") {
                        let (code, filename) = read_input(Some(path))?;
                        job_inputs += usize::from(push_explicit_source(
                            &mut job,
                            wakaru::Source::new(filename, code),
                            prefix_gtm_pieces,
                            &mut gtm,
                        )?);
                        pushed_inputs += 1;
                        continue;
                    }
                    for source in read_explicit_unpack_sources(path)? {
                        job_inputs += usize::from(push_explicit_source(
                            &mut job,
                            source,
                            prefix_gtm_pieces,
                            &mut gtm,
                        )?);
                        pushed_inputs += 1;
                    }
                    continue;
                }

                for candidate in collect_directory_js_inputs(path)? {
                    stats.scanned += 1;
                    let code = fs::read_to_string(&candidate)
                        .with_context(|| format!("failed to read {}", candidate.display()))?;
                    let receipt = match job.push_with_unmatched(
                        wakaru::Source::new(candidate.to_string_lossy().to_string(), code),
                        wakaru::UnmatchedInput::Skip,
                    ) {
                        Ok(receipt) => receipt,
                        Err(error) if error.kind() == wakaru::ErrorKind::Parse => {
                            stats.skipped += 1;
                            continue;
                        }
                        Err(error) => return Err(error.into()),
                    };
                    pushed_inputs += 1;
                    job_inputs += 1;
                    if receipt.detection == wakaru::InputDetection::Plain {
                        stats.skipped += 1;
                    } else {
                        stats.detected += 1;
                    }
                }
            }
        }
    }

    if saw_directory && pushed_inputs == 0 {
        bail!("no bundle or chunk files detected in directory input");
    }
    let (mut output, single_input_name) = if job_inputs == 0 && !gtm.is_empty() {
        let empty = CliUnpackOutput {
            modules: Vec::new(),
            provenance: Vec::new(),
            warnings: Vec::new(),
            detected_formats: Vec::new(),
            source_maps: Vec::new(),
            safety: wakaru::OutputSafety::Normal,
        };
        (empty, None)
    } else {
        let output = {
            let span = tracing::info_span!("cli_unpack_finish");
            let _enter = span.enter();
            job.finish()?
        };
        if saw_directory && output.modules.is_empty() {
            bail!("no bundle or chunk files detected in directory input");
        }
        let single_input_name =
            (output.inputs.len() == 1).then(|| output.inputs[0].filename.clone());
        (adapt_public_unpack_output(output), single_input_name)
    };
    let (gtm_modules, gtm_warnings) = decompile_gtm_scripts(&gtm, raw, rewrite, diagnostics);
    output.modules.extend(gtm_modules);
    output.warnings.extend(gtm_warnings);
    Ok(PublicUnpackExecution {
        output,
        scan_stats: (stats.scanned > 0).then_some(stats),
        single_input_name,
        gtm,
    })
}

/// A Google Tag Manager container read from one input, kept for the index written next to its
/// pieces.
struct GtmIntake {
    input: String,
    /// Directory the pieces and `index.md` are written under, relative to the output directory.
    prefix: String,
    container: wakaru::gtm::Container,
}

/// Pushes an explicit input into the job, unless it is a Google Tag Manager container: then it is
/// set aside, and its scripts are decompiled after the job (see [`decompile_gtm_scripts`]). Only
/// inputs that carry the container's keys are parsed a second time.
fn push_explicit_source(
    job: &mut wakaru::UnpackJob,
    source: wakaru::Source,
    prefix_pieces: bool,
    gtm: &mut Vec<GtmIntake>,
) -> Result<bool> {
    let Some(container) = wakaru::gtm::extract_container(source.code()) else {
        job.push(source)?;
        return Ok(true);
    };
    let input = source.filename().to_string();
    let prefix = if prefix_pieces {
        // the file name alone: an input path can be absolute, and output paths must stay under
        // the output directory
        let stem = Path::new(&input).file_stem().map_or_else(
            || "gtm".to_string(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        let mut prefix = format!("{stem}/");
        let mut suffix = 1;
        while gtm.iter().any(|intake| intake.prefix == prefix) {
            suffix += 1;
            prefix = format!("{stem}-{suffix}/");
        }
        prefix
    } else {
        String::new()
    };
    gtm.push(GtmIntake {
        input,
        prefix,
        container,
    });
    Ok(false)
}

/// Decompiles every container script on its own, as a single file: a script is not a bundle, and
/// bundle detection would split an ordinary IIFE by heuristic. A script that does not parse is kept
/// as extracted and reported as `decompile_failed`, like any module that failed to decompile.
fn decompile_gtm_scripts(
    gtm: &[GtmIntake],
    raw: bool,
    rewrite: wakaru::RewriteOptions,
    diagnostics: bool,
) -> (Vec<(String, String)>, Vec<CliWarning>) {
    let pieces: Vec<(String, &str)> = gtm
        .iter()
        .flat_map(|intake| {
            intake.container.scripts.iter().map(|script| {
                (
                    format!("{}{}", intake.prefix, gtm_script_path(script)),
                    script.code.as_str(),
                )
            })
        })
        .collect();
    let decompiled: Vec<(String, String, Vec<CliWarning>)> = pieces
        .into_par_iter()
        .map(|(filename, code)| {
            if raw {
                return (filename, code.to_string(), Vec::new());
            }
            let options = wakaru::DecompileOptions::default()
                .with_rewrite(rewrite)
                .with_diagnostics(diagnostics);
            match wakaru::decompile(wakaru::Source::new(filename.clone(), code), options) {
                Ok(output) => {
                    let output = adapt_public_decompile_output(output);
                    (filename, output.code, output.warnings)
                }
                Err(error) => {
                    let warning = CliWarning::new(
                        filename.clone(),
                        wakaru::DiagnosticCode::DecompileFailed,
                        wakaru::DiagnosticSeverity::Error,
                        error.to_string(),
                    );
                    (filename, code.to_string(), vec![warning])
                }
            }
        })
        .collect();
    let mut modules = Vec::with_capacity(decompiled.len());
    let mut warnings: Vec<CliWarning> = gtm.iter().flat_map(gtm_warnings).collect();
    for (filename, code, piece_warnings) in decompiled {
        modules.push((filename, code));
        warnings.extend(piece_warnings);
    }
    (modules, warnings)
}

fn gtm_script_path(script: &wakaru::gtm::Script) -> String {
    match script.field {
        wakaru::gtm::Field::CustomJavaScriptVariable { macro_index } => {
            format!("macros/jsm-{macro_index}.js")
        }
        wakaru::gtm::Field::CustomHtmlTag { tag_id } => {
            format!("tags/html-{tag_id}-{}.js", script.position)
        }
        _ => format!("scripts/script-{}.js", script.position),
    }
}

fn gtm_field_label(field: wakaru::gtm::Field) -> String {
    match field {
        wakaru::gtm::Field::CustomJavaScriptVariable { macro_index } => {
            format!("Custom JS variable {macro_index}")
        }
        wakaru::gtm::Field::CustomHtmlTag { tag_id } => format!("Custom HTML tag {tag_id}"),
        _ => "container field".to_string(),
    }
}

/// Escape modes or template parts the extractor reports but does not rewrite. Not an error: the
/// reference stays in the code as a placeholder identifier.
const GTM_UNSUPPORTED_ESCAPE_KIND: &str = "gtm_unsupported_escape";

fn gtm_warnings(intake: &GtmIntake) -> impl Iterator<Item = CliWarning> + '_ {
    intake.container.unsupported_parts.iter().map(|part| {
        let message = match part.macro_index {
            Some(index) => format!(
                "{}: macro {index} uses escape mode {}, which is not understood",
                gtm_field_label(part.field),
                part.modes
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => format!(
                "{}: template part that is neither text nor a macro reference",
                gtm_field_label(part.field)
            ),
        };
        CliWarning {
            filename: intake.input.clone(),
            kind: GTM_UNSUPPORTED_ESCAPE_KIND.to_string(),
            is_error: false,
            message,
        }
    })
}

/// The `index.md` written next to a container's pieces: what each `__gtm_macro_<n>` is, the tags
/// with the triggers that fire or block them, those triggers' conditions decoded, and the URLs of
/// the external scripts, which have no body to write.
fn gtm_index_markdown(container: &wakaru::gtm::Container) -> String {
    let mut out = String::from("# Google Tag Manager container\n\n");
    if let Some(version) = &container.version {
        out.push_str(&format!("Container version: `{version}`\n"));
    }
    out.push_str(
        "\nCustom JS variables and Custom HTML scripts were written next to this file. Macro\n\
         references read `__gtm_macro_<n>`, where `<n>` is the index in the table below.\n\n\
         ## Macros\n\n| # | function | name |\n|---|---|---|\n",
    );
    for m in &container.macros {
        out.push_str(&format!(
            "| {} | `{}` | {} |\n",
            m.index,
            gtm_cell(&m.function),
            gtm_cell(m.name.as_deref().unwrap_or(""))
        ));
    }

    out.push_str("\n## Tags\n\n| tag_id | type | fired by | blocked by |\n|---|---|---|---|\n");
    for tag in &container.tags {
        out.push_str(&format!(
            "| {} | `{}` | {} | {} |\n",
            tag.tag_id,
            gtm_cell(&tag.function),
            gtm_trigger_indices(container, tag.tag_id, true),
            gtm_trigger_indices(container, tag.tag_id, false)
        ));
    }

    out.push_str(
        "\n## Triggers\n\nEach trigger's condition and the tags it fires or blocks. A reference is the\n\
         macro's name, or its type and component when it has no name (`__e`, `__u:PATH`), followed\n\
         by its row in the macro table when a macro with a different definition reads the same\n\
         (`__u:QUERY#4`); a repeated definition reads as its first row. A\n\
         `{{…}}` inside a quoted value is a macro joined into the text.\n\n\
         | # | when | fires | blocks |\n|---|---|---|---|\n",
    );
    for trigger in &container.triggers {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            trigger.index,
            gtm_trigger_condition(trigger, container),
            gtm_tag_ids(&trigger.fires),
            gtm_tag_ids(&trigger.blocks)
        ));
    }

    out.push_str("\n## External scripts\n\n");
    if container.external_scripts.is_empty() {
        out.push_str("None.\n");
    } else {
        out.push_str("| tag | url |\n|---|---|\n");
        for script in &container.external_scripts {
            out.push_str(&format!(
                "| {} | {} |\n",
                script.tag_id,
                gtm_cell(&script.url)
            ));
        }
    }
    out
}

/// The rows of the `Triggers` table that fire (`firing`) or block a tag.
fn gtm_trigger_indices(container: &wakaru::gtm::Container, tag_id: u64, firing: bool) -> String {
    container
        .triggers
        .iter()
        .filter(|trigger| {
            let tags = if firing {
                &trigger.fires
            } else {
                &trigger.blocks
            };
            tags.contains(&tag_id)
        })
        .map(|trigger| trigger.index.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn gtm_tag_ids(ids: &[u64]) -> String {
    ids.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A trigger's condition, decoded: the `when` predicates must all hold and no `unless` predicate
/// may. An empty group is dropped, so a block-only trigger reads `not …` rather than `and not …`.
fn gtm_trigger_condition(
    trigger: &wakaru::gtm::Trigger,
    container: &wakaru::gtm::Container,
) -> String {
    let when = gtm_predicates(&trigger.when, container);
    let unless = gtm_predicates(&trigger.unless, container);
    let text = match (when.is_empty(), unless.is_empty()) {
        (false, false) => format!(
            "{} and not {}",
            when.join(" and "),
            gtm_predicate_group(&unless)
        ),
        (false, true) => when.join(" and "),
        (true, false) => format!("not {}", gtm_predicate_group(&unless)),
        (true, true) => String::from("always"),
    };
    // a `|` inside a value (a regular expression alternation, most often) would end the table cell
    gtm_cell(&text)
}

/// A Markdown table cell: a `|` would end the cell, and a line break the row.
fn gtm_cell(text: &str) -> String {
    let mut out = text.replace('|', "\\|");
    for line_break in ['\n', '\r', '\u{2028}', '\u{2029}'] {
        out = out.replace(line_break, " ");
    }
    out
}

/// A group of `unless` predicates, which hold together as one negation.
fn gtm_predicate_group(parts: &[String]) -> String {
    if parts.len() == 1 {
        parts[0].clone()
    } else {
        format!("({})", parts.join(" or "))
    }
}

fn gtm_predicates(indices: &[usize], container: &wakaru::gtm::Container) -> Vec<String> {
    indices
        .iter()
        .map(
            |index| match container.predicates.iter().find(|p| p.index == *index) {
                Some(predicate) => format!("`{}`", gtm_predicate(predicate, container)),
                // an index outside the array is a malformed container: name it rather than drop it, or
                // the condition would read as if the comparison were never there
                None => format!("`predicate {index}`"),
            },
        )
        .collect()
}

/// One predicate in words. An operator the table does not name keeps its GTM name rather than a
/// guess, the same way an unknown escape mode keeps its identifier.
fn gtm_predicate(predicate: &wakaru::gtm::Predicate, container: &wakaru::gtm::Container) -> String {
    let operator = match predicate.function.as_str() {
        "_eq" => "equals",
        "_cn" => "contains",
        "_sw" => "starts with",
        "_ew" => "ends with",
        "_re" => "matches",
        "_gt" => "is greater than",
        "_lt" => "is less than",
        "_ge" => "is greater than or equal to",
        "_le" => "is less than or equal to",
        "_css" => "matches the CSS selector",
        other => other,
    };
    let mut text = format!(
        "{} {} {}",
        gtm_operand(&predicate.arg0, container),
        operator,
        gtm_operand_comparison(predicate, container)
    );
    // the runtime reads `ignore_case` only for `_re`, so on another comparison it changes nothing
    if predicate.ignore_case && predicate.function == "_re" {
        text.push_str(" (ignoring case)");
    }
    text
}

/// The right-hand operand. When `any_of` is set, a list holds alternatives rather than one value,
/// so its elements read as `or` instead of a comma list.
fn gtm_operand_comparison(
    predicate: &wakaru::gtm::Predicate,
    container: &wakaru::gtm::Container,
) -> String {
    match (&predicate.arg1, predicate.any_of) {
        (wakaru::gtm::Operand::List(items), true) => gtm_operand_list(items, container, " or "),
        (other, _) => gtm_operand(other, container),
    }
}

fn gtm_operand(operand: &wakaru::gtm::Operand, container: &wakaru::gtm::Container) -> String {
    match operand {
        wakaru::gtm::Operand::Value(text) => format!("\"{text}\""),
        wakaru::gtm::Operand::Macro { index } => gtm_macro_reference(*index, container),
        wakaru::gtm::Operand::List(items) => gtm_operand_list(items, container, ", "),
        wakaru::gtm::Operand::Template(parts) => {
            let text: String = parts
                .iter()
                .map(|part| match part {
                    wakaru::gtm::Operand::Value(text) => text.clone(),
                    wakaru::gtm::Operand::Macro { index } => {
                        format!("{{{{{}}}}}", gtm_macro_reference(*index, container))
                    }
                    other => format!("{{{{{}}}}}", gtm_operand(other, container)),
                })
                .collect();
            format!("\"{text}\"")
        }
        // `Operand` is non-exhaustive: show a variant this build does not know instead of dropping it
        other => format!("{other:?}"),
    }
}

fn gtm_operand_list(
    items: &[wakaru::gtm::Operand],
    container: &wakaru::gtm::Container,
    separator: &str,
) -> String {
    let items: Vec<String> = items
        .iter()
        .map(|item| gtm_operand(item, container))
        .collect();
    format!("({})", items.join(separator))
}

/// How the macro at `index` reads inside a condition: its label, followed by `#<row>` when a macro
/// with a different definition has the same label. Two `__u:QUERY` variables differ only in their
/// query key, and two unnamed `__jsm` variables only in their body, so the label alone does not say
/// which one is meant. A repeated definition reads as its first copy, so copies of one variable keep
/// the bare label and, when they need a row, share it.
fn gtm_macro_reference(index: u64, container: &wakaru::gtm::Container) -> String {
    let Some(mac) = container.macros.iter().find(|m| m.index as u64 == index) else {
        return format!("__gtm_macro_{index}");
    };
    let first = mac.same_as.unwrap_or(mac.index);
    let label = gtm_macro_label(mac);
    let shared = container.macros.iter().any(|other| {
        other.same_as.is_none() && other.index != first && gtm_macro_label(other) == label
    });
    if shared {
        format!("{label}#{first}")
    } else {
        label
    }
}

/// A macro's `name`, or its type and component when it has none (`__e`, `__u:PATH`), so two
/// built-ins that share a type but not a component do not read the same.
fn gtm_macro_label(mac: &wakaru::gtm::Macro) -> String {
    if let Some(name) = &mac.name {
        return name.clone();
    }
    match &mac.component {
        Some(component) => format!("{}:{}", mac.function, component),
        None => mac.function.clone(),
    }
}

fn read_explicit_unpack_sources(path: &Path) -> Result<Vec<wakaru::Source>> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    if !is_executable_container(&bytes) {
        let code = String::from_utf8(bytes)
            .with_context(|| format!("failed to read {} as UTF-8", path.display()))?;
        return Ok(vec![wakaru::Source::new(
            path.to_string_lossy().to_string(),
            code,
        )]);
    }

    let standalone = wakaru::bun::extract_standalone(&bytes)
        .with_context(|| {
            format!(
                "failed to extract Bun single-file executable {}",
                path.display()
            )
        })?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} is an executable but does not contain a supported Bun standalone graph",
                path.display()
            )
        })?;
    let mut javascript = standalone
        .files
        .iter()
        .filter(|file| file.is_javascript_like())
        .collect::<Vec<_>>();
    javascript.sort_by_key(|file| (!file.is_entry, file.index));
    if javascript.is_empty() {
        bail!(
            "Bun single-file executable {} contains no JavaScript-like embedded files",
            path.display()
        );
    }

    javascript
        .into_iter()
        .map(|file| {
            let code = std::str::from_utf8(file.contents).with_context(|| {
                format!(
                    "Bun embedded JavaScript {:?} in {} is not UTF-8",
                    file.name,
                    path.display()
                )
            })?;
            let embedded = sanitize_bun_embedded_path(&file.name, file.index);
            Ok(wakaru::Source::new(
                format!("{}#bun/{embedded}", path.to_string_lossy()),
                code,
            ))
        })
        .collect()
}

fn is_executable_container(bytes: &[u8]) -> bool {
    const MAGICS: [&[u8]; 10] = [
        b"MZ",
        b"\x7fELF",
        b"\xfe\xed\xfa\xce",
        b"\xfe\xed\xfa\xcf",
        b"\xce\xfa\xed\xfe",
        b"\xcf\xfa\xed\xfe",
        b"\xca\xfe\xba\xbe",
        b"\xca\xfe\xba\xbf",
        b"\xbe\xba\xfe\xca",
        b"\xbf\xba\xfe\xca",
    ];
    MAGICS.iter().any(|magic| bytes.starts_with(magic))
}

fn sanitize_bun_embedded_path(name: &str, index: u32) -> String {
    let normalized = name.replace('\\', "/");
    let without_prefix = ["/$bunfs/root/", "/$bunfs/", "B:/~BUN/root/", "B:/~BUN/"]
        .into_iter()
        .find_map(|prefix| normalized.strip_prefix(prefix))
        .unwrap_or(&normalized);
    let mut sanitized = without_prefix
        .split('/')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .collect::<Vec<_>>()
        .join("/");
    if sanitized.is_empty() {
        sanitized = format!("embedded-{index}.js");
    } else if Path::new(&sanitized).extension().is_none() {
        sanitized.push_str(".js");
    }
    sanitized
}

fn public_rewrite_level(level: RewriteLevel) -> wakaru::RewriteLevel {
    match level {
        RewriteLevel::Minimal => wakaru::RewriteLevel::Minimal,
        RewriteLevel::Standard => wakaru::RewriteLevel::Standard,
        RewriteLevel::Aggressive => wakaru::RewriteLevel::Aggressive,
    }
}

fn public_unpack_mode(mode: UnpackMode) -> wakaru::UnpackMode {
    match mode {
        UnpackMode::Auto => wakaru::UnpackMode::Auto,
        UnpackMode::Strict => wakaru::UnpackMode::Strict,
        UnpackMode::Inspect => wakaru::UnpackMode::Inspect,
    }
}

fn public_dce_mode(mode: DceMode) -> wakaru::DceMode {
    match mode {
        DceMode::Off => wakaru::DceMode::Off,
        DceMode::TransformOnly => wakaru::DceMode::TransformOnly,
        DceMode::Full => wakaru::DceMode::Full,
    }
}

fn adapt_public_unpack_output(output: wakaru::UnpackOutput) -> CliUnpackOutput {
    let span = tracing::info_span!("cli_adapt_public_unpack_output");
    let _enter = span.enter();
    let safety = output.safety;
    let input_names = output
        .inputs
        .iter()
        .map(|input| input.filename.as_str())
        .collect::<Vec<_>>();
    let detected_formats = output
        .inputs
        .iter()
        .filter_map(|input| match input.detection {
            wakaru::InputDetection::Structural(format) => Some(CliBundleFormat::Structural(format)),
            wakaru::InputDetection::HeuristicScopeHoisted => Some(CliBundleFormat::ScopeHoisted),
            wakaru::InputDetection::Plain => None,
            _ => None,
        })
        .fold(Vec::new(), |mut formats, format| {
            if !formats.contains(&format) {
                formats.push(format);
            }
            formats
        });
    let warnings = output
        .diagnostics
        .into_iter()
        .map(|diagnostic| {
            let filename = diagnostic
                .module
                .and_then(|index| output.modules.get(index))
                .map(|module| module.filename.clone())
                .or_else(|| {
                    diagnostic
                        .input
                        .and_then(|id| input_names.get(id.get() as usize))
                        .map(|name| (*name).to_string())
                })
                .unwrap_or_default();
            CliWarning::from_public(diagnostic, filename)
        })
        .collect();
    // A module with no spans (an empty factory body) still names its input
    // through the reports. A module listed by several inputs gets none.
    let mut report_inputs: Vec<Option<Option<&str>>> = vec![None; output.modules.len()];
    for input in &output.inputs {
        for &index in &input.module_indices {
            if let Some(slot) = report_inputs.get_mut(index) {
                *slot = match slot {
                    None => Some(Some(input.filename.as_str())),
                    Some(_) => Some(None),
                };
            }
        }
    }
    let provenance = output
        .modules
        .iter()
        .enumerate()
        .map(|(index, module)| {
            let input = module
                .provenance
                .first()
                .and_then(|span| input_names.get(span.input.get() as usize))
                .copied()
                .or_else(|| report_inputs[index].flatten())
                .unwrap_or_default()
                .to_string();
            CliModuleProvenance {
                filename: module.filename.clone(),
                input,
                ranges: module
                    .provenance
                    .iter()
                    .map(|span| (span.start, span.end))
                    .collect(),
                inspection_context_ranges: module
                    .inspection_context
                    .iter()
                    .map(|span| (span.start, span.end))
                    .collect(),
            }
        })
        .collect();
    let source_maps = output
        .modules
        .iter()
        .filter_map(|module| {
            module
                .source_map
                .as_ref()
                .map(|map| (module.filename.clone(), map.clone()))
        })
        .collect();
    let modules = output
        .modules
        .into_iter()
        .map(|module| (module.filename, module.code))
        .collect();
    CliUnpackOutput {
        modules,
        provenance,
        warnings,
        detected_formats,
        source_maps,
        safety,
    }
}

/// Append `.map` to a path's extension: `foo.js` → `foo.js.map`.
fn append_map_extension(path: &Path) -> PathBuf {
    let mut map_name = path.as_os_str().to_owned();
    map_name.push(".map");
    PathBuf::from(map_name)
}

/// Write an output source map next to its JavaScript file.
///
/// Source map consumers resolve `sources` against the map's own location,
/// while the engine names inputs as the CLI received them (relative to the
/// working directory). Rewrite each source that names a file on disk to a
/// path relative to the map; others (stdin, executable members) stay as-is.
fn write_output_source_map(map_path: &Path, map_json: &str) -> Result<()> {
    write_file(map_path, &relativize_map_sources(map_path, map_json))
}

fn relativize_map_sources(map_path: &Path, map_json: &str) -> String {
    #[derive(serde::Deserialize)]
    struct MapSources {
        sources: Vec<String>,
    }

    // Only `sources` changes; leave the rest (including the large
    // `mappings` string) untouched instead of decoding and re-encoding it.
    let Ok(MapSources { sources }) = serde_json::from_str::<MapSources>(map_json) else {
        return map_json.to_string();
    };
    let map_dir = match map_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let Ok(map_dir) = fs::canonicalize(map_dir) else {
        return map_json.to_string();
    };
    let relativized: Vec<String> = sources
        .iter()
        .map(|source| {
            let source_path = Path::new(source);
            source_path
                .is_file()
                .then(|| fs::canonicalize(source_path).ok())
                .flatten()
                .and_then(|source| relative_path(&map_dir, &source))
                .unwrap_or_else(|| source.clone())
        })
        .collect();
    if relativized == sources {
        return map_json.to_string();
    }
    let (Ok(old), Ok(new)) = (
        serde_json::to_string(&sources),
        serde_json::to_string(&relativized),
    ) else {
        return map_json.to_string();
    };
    // The engine writes compact JSON, so the array appears exactly as serde
    // serializes it.
    map_json.replacen(
        &format!("\"sources\":{old}"),
        &format!("\"sources\":{new}"),
        1,
    )
}

/// Slash-separated path from `from_dir` to `to`, both absolute. `None` when
/// they share no root (for example different Windows drives).
fn relative_path(from_dir: &Path, to: &Path) -> Option<String> {
    let from: Vec<_> = from_dir.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(from, to)| from == to)
        .count();
    if common == 0 {
        return None;
    }
    let parts: Vec<String> = std::iter::repeat_n("..".to_string(), from.len() - common)
        .chain(
            to[common..]
                .iter()
                .map(|part| part.as_os_str().to_string_lossy().into_owned()),
        )
        .collect();
    Some(parts.join("/"))
}

fn ensure_output_file(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "output file {} already exists; pass --force to overwrite",
            path.display()
        );
    }
    Ok(())
}

/// Ensures an output directory is usable.
///
/// Returns true when the directory already contained entries and writes should
/// preserve the read-before-write unchanged-file fast path.
fn ensure_output_dir(path: &PathBuf, force: bool) -> Result<bool> {
    if path.exists() {
        if !path.is_dir() {
            bail!(
                "output path {} exists and is not a directory",
                path.display()
            );
        }
        let is_empty = path
            .read_dir()
            .with_context(|| format!("failed to read output directory {}", path.display()))?
            .next()
            .is_none();
        if !is_empty && !force {
            bail!(
                "output directory {} is not empty; pass --force to write into it",
                path.display()
            );
        }
        return Ok(!is_empty);
    } else {
        fs::create_dir_all(path)
            .with_context(|| format!("failed to create output directory {}", path.display()))?;
    }
    Ok(false)
}

/// Render provenance entries as a JSON document.
///
/// `final_names` maps the driver's module filename to the relative path the
/// CLI actually wrote (CLI-side dedup can rename). `default_input` fills in
/// entries whose input is empty (single-source unpacks).
fn render_provenance_json(
    provenance: &[CliModuleProvenance],
    final_names: &HashMap<&str, String>,
    default_input: &str,
    detected_formats: &[CliBundleFormat],
) -> String {
    let mut entries: Vec<(String, &CliModuleProvenance)> = provenance
        .iter()
        .map(|entry| {
            let name = final_names
                .get(entry.filename.as_str())
                .cloned()
                .unwrap_or_else(|| entry.filename.clone());
            (name, entry)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let format = provenance_format(detected_formats);
    let strategy = provenance_strategy(&entries, detected_formats);

    let mut json = format!(
        "{{\n  \"format\": \"{}\",\n  \"strategy\": \"{}\",\n  \"modules\": {{\n",
        json_escape(format),
        strategy,
    );
    for (i, (name, entry)) in entries.iter().enumerate() {
        let input = if entry.input.is_empty() {
            default_input
        } else {
            &entry.input
        };
        let ranges = entry
            .ranges
            .iter()
            .map(|(start, end)| format!("[{start},{end}]"))
            .collect::<Vec<_>>()
            .join(",");
        let extraction = provenance_extraction(name, strategy);
        let context_ranges = if entry.inspection_context_ranges.is_empty() {
            String::new()
        } else {
            let ranges = entry
                .inspection_context_ranges
                .iter()
                .map(|(start, end)| format!("[{start},{end}]"))
                .collect::<Vec<_>>()
                .join(",");
            format!(", \"context_ranges\": [{ranges}]")
        };
        json.push_str(&format!(
            "    \"{}\": {{\"input\": \"{}\", \"ranges\": [{}], \"extraction\": \"{}\"{}}}{}\n",
            json_escape(name),
            json_escape(input),
            ranges,
            extraction,
            context_ranges,
            if i + 1 < entries.len() { "," } else { "" }
        ));
    }
    json.push_str("  }\n}\n");
    json
}

fn provenance_final_names<'a>(
    artifacts: &'a [CliOutputArtifact],
    resolved: &[(PathBuf, &str)],
    out_dir: &Path,
) -> HashMap<&'a str, String> {
    let mut final_names = HashMap::new();
    for (artifact, (path, _)) in artifacts.iter().zip(resolved.iter()) {
        if artifact.kind != JsonModuleKind::JavaScript {
            continue;
        }
        if let Some(source_filename) = artifact.source_map_filename.as_deref() {
            let relative = path
                .strip_prefix(out_dir)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            final_names.insert(source_filename, relative);
        }
    }
    final_names
}

fn provenance_format(detected_formats: &[CliBundleFormat]) -> &'static str {
    let mut non_scope = detected_formats
        .iter()
        .copied()
        .filter(|format| !format.is_scope_hoisted());
    let Some(first) = non_scope.next() else {
        return detected_formats
            .first()
            .map(|format| format.as_str())
            .unwrap_or("unknown");
    };
    if non_scope.any(|format| format != first) {
        "unknown"
    } else {
        first.as_str()
    }
}

fn provenance_strategy(
    entries: &[(String, &CliModuleProvenance)],
    detected_formats: &[CliBundleFormat],
) -> &'static str {
    let has_scope_format = detected_formats
        .iter()
        .any(|format| format.is_scope_hoisted());
    let has_structural_format = detected_formats
        .iter()
        .any(|format| !format.is_scope_hoisted());
    let has_heuristic_modules = entries
        .iter()
        .any(|(name, _)| is_heuristic_provenance_module(name));

    if has_scope_format && !has_structural_format {
        "heuristic"
    } else if has_heuristic_modules || (has_scope_format && has_structural_format) {
        "mixed"
    } else {
        "structural"
    }
}

fn provenance_extraction(name: &str, strategy: &str) -> &'static str {
    if strategy == "heuristic" || is_heuristic_provenance_module(name) {
        "heuristic"
    } else {
        "structural"
    }
}

fn is_heuristic_provenance_module(name: &str) -> bool {
    name.starts_with("chunk_") || name.contains("/chunk_")
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn init_profile(
    path: Option<&Path>,
    include_rule_spans: bool,
) -> Result<Option<tracing_chrome::FlushGuard>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let file = fs::File::create(path)
        .with_context(|| format!("failed to create profile file {}", path.display()))?;

    let (chrome_layer, guard) = tracing_chrome::ChromeLayerBuilder::new()
        .writer(file)
        .include_args(true)
        .build();
    let level = if include_rule_spans {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    tracing_subscriber::registry()
        .with(chrome_layer.with_filter(level))
        .try_init()
        .context("failed to initialize profiling subscriber")?;

    Ok(Some(guard))
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
