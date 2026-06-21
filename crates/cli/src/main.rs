use std::collections::BTreeSet;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use rayon::prelude::*;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;
use wakaru_core::{
    decompile, extract_source_entries, format_trace_events, normalize, parse_sourcemap,
    trace_rules, unpack, unpack_files, unpack_files_raw, unpack_raw, DceMode, DecompileOptions,
    NormalizeOptions, RewriteLevel, RuleTraceOptions, UnpackInput,
};

mod color;
mod discovery;
mod formatter;
mod json_output;
mod output;

use color::Styled;
use discovery::{scan_directory_for_unpack_inputs, DirectoryScanStats};
use formatter::{format_cli_output, selected_formatter};
use json_output::{JsonDecompileOutput, JsonModule, JsonUnpackOutput, JsonWarning};
use output::{canonicalize_output_dir, resolve_unpack_output_path, write_file, write_if_changed};

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

    /// Input JavaScript/TypeScript file(s). With --unpack, directories are
    /// scanned recursively for bundle/chunk files.
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
    #[arg(short, long, value_enum, num_args = 0..=1, default_missing_value = "auto")]
    unpack: Option<UnpackMode>,

    /// With --unpack, write raw unpacker output before the decompiler rule pipeline.
    #[arg(long, requires = "unpack")]
    raw: bool,

    /// Source map file (.map) for identifier recovery and import deduplication.
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

    /// Run a final formatter pass on decompiled output.
    #[arg(long)]
    formatter: bool,

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let _profile_guard = init_profile(cli.profile.as_deref(), cli.profile_rules)?;

    match cli.command.clone() {
        Some(Command::Extract(args)) => run_extract(args, cli.force),
        Some(Command::Debug(args)) => run_debug(args, cli.force),
        None => run_default(cli),
    }
}

fn run_default(cli: Cli) -> Result<()> {
    if cli.unpack.is_some() && cli.output.is_none() {
        bail!("--unpack requires -o/--output to choose an output directory");
    }

    let heuristic_split = !matches!(cli.unpack, Some(UnpackMode::Strict));
    let formatter = selected_formatter(cli.formatter);
    let styled = if cli.json {
        Styled::off()
    } else {
        Styled::for_stderr()
    };

    if cli.unpack.is_some() {
        let input_set = read_unpack_inputs(&cli.inputs, heuristic_split)?;
        let scan_stats = input_set.scan_stats;
        let inputs = input_set.inputs;
        if inputs.len() > 1 && cli.sourcemap.is_some() {
            bail!("--source-map is only supported with a single input file");
        }
        let sourcemap_bytes = read_sourcemap(cli.sourcemap.as_ref())?;
        let filename = inputs
            .first()
            .map(|input| input.filename.clone())
            .unwrap_or_default();
        let dce_mode = if cli.dce {
            DceMode::Full
        } else {
            DceMode::TransformOnly
        };
        let options = DecompileOptions {
            filename,
            sourcemap: sourcemap_bytes,
            dce_mode,
            level: cli.level.into(),
            heuristic_split,
            diagnostics: cli.diagnostics,
        };

        let out_dir = cli.output.expect("checked above");
        let check_existing_writes = ensure_output_dir(&out_dir, cli.force)?;
        let out_dir = canonicalize_output_dir(&out_dir)?;

        let start = Instant::now();
        let output = if inputs.len() == 1 {
            let input = inputs.into_iter().next().expect("checked input length");
            if cli.raw {
                unpack_raw(&input.source, &options)?
            } else {
                unpack(&input.source, options)?
            }
        } else if cli.raw {
            unpack_files_raw(inputs, &options)?
        } else {
            unpack_files(inputs, options)?
        };
        let elapsed = start.elapsed();

        if !cli.json {
            print_warnings(&output.warnings, &styled);
        }
        let error_modules: Vec<&str> = output
            .warnings
            .iter()
            .filter(|w| w.kind.is_error())
            .map(|w| w.filename.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        let pairs = output.modules;
        let pairs: Vec<(String, String)> = pairs
            .into_par_iter()
            .map(|(filename, code)| {
                let formatted = format_cli_output(code, &filename, formatter);
                (filename, formatted)
            })
            .collect();

        let resolved: Vec<(PathBuf, &str)> = {
            let span = tracing::info_span!("cli_resolve_output_paths");
            let _enter = span.enter();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            pairs
                .iter()
                .map(|(filename, code)| {
                    let out_path = resolve_unpack_output_path(&out_dir, filename, &mut seen)?;
                    Ok((out_path, code.as_str()))
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

        if cli.json {
            let json = JsonUnpackOutput {
                detected_formats: output
                    .detected_formats
                    .iter()
                    .map(|f| f.as_str().to_string())
                    .collect(),
                modules: pairs
                    .iter()
                    .map(|(filename, _)| JsonModule {
                        filename: filename.clone(),
                    })
                    .collect(),
                warnings: output.warnings.iter().map(JsonWarning::from_core).collect(),
                total: resolved.len(),
                failed: error_modules.len(),
                elapsed_ms: elapsed.as_millis() as u64,
            };
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
            let fail_info = if error_modules.is_empty() {
                String::new()
            } else {
                format!(" ({} failed)", error_modules.len())
            };
            eprintln!(
                "total: {} module(s){fail_info} in {}",
                styled.bold(&resolved.len().to_string()),
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
    } else {
        if cli.inputs.len() > 1 {
            bail!("multiple input files require --unpack");
        }
        if let Some(input) = cli.inputs.first() {
            if input.is_dir() {
                bail!("cannot decompile a directory. Pass a JavaScript file or use --unpack");
            }
        }
        let (input, filename) = read_input(cli.inputs.first())?;
        let output_filename = filename.clone();
        let sourcemap_bytes = read_sourcemap(cli.sourcemap.as_ref())?;
        let dce_mode = if cli.dce {
            DceMode::Full
        } else {
            DceMode::TransformOnly
        };
        let options = DecompileOptions {
            filename,
            sourcemap: sourcemap_bytes,
            dce_mode,
            level: cli.level.into(),
            heuristic_split,
            diagnostics: cli.diagnostics,
        };

        let start = Instant::now();
        let output = decompile(&input, options)?;
        let elapsed = start.elapsed();

        if !cli.json {
            print_warnings(&output.warnings, &styled);
        }
        let has_errors = output.has_errors();
        let code = format_cli_output(output.code, &output_filename, formatter);

        if cli.json {
            let json_code = if cli.output.is_none() {
                Some(code.clone())
            } else {
                None
            };
            if let Some(ref path) = cli.output {
                ensure_output_file(path, cli.force)?;
                fs::write(path, &code)
                    .with_context(|| format!("failed to write {}", path.display()))?;
            }
            let json = JsonDecompileOutput {
                code: json_code,
                warnings: output.warnings.iter().map(JsonWarning::from_core).collect(),
                elapsed_ms: elapsed.as_millis() as u64,
            };
            println!(
                "{}",
                serde_json::to_string(&json).expect("JSON serialization")
            );
        } else {
            match cli.output {
                Some(path) => {
                    ensure_output_file(&path, cli.force)?;
                    fs::write(&path, &code)
                        .with_context(|| format!("failed to write {}", path.display()))?;
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
                .filter(|w| w.kind.is_error())
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
    }

    Ok(())
}

fn print_warnings(warnings: &[wakaru_core::UnpackWarning], styled: &Styled) {
    for warning in warnings {
        let label = if warning.kind.is_error() {
            styled.error("error")
        } else {
            styled.warning("warning")
        };
        eprintln!("{label}: {warning}");
    }
}

fn format_elapsed(d: Duration) -> String {
    if d.as_secs() >= 1 {
        format!("{:.2}s", d.as_secs_f64())
    } else {
        format!("{}ms", d.as_millis())
    }
}

fn run_extract(args: ExtractArgs, force: bool) -> Result<()> {
    let map_bytes = fs::read(&args.map)
        .with_context(|| format!("failed to read source map {}", args.map.display()))?;
    let sm = parse_sourcemap(&map_bytes)?;
    ensure_output_dir(&args.output, force)?;

    let entries = extract_source_entries(&sm, &args.output);
    let mut written = 0;
    for (out_path, content) in &entries {
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(out_path, content)
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
    }
}

fn run_normalize(args: NormalizeArgs) -> Result<()> {
    let (source, filename) = read_input(args.input.as_ref())?;
    let options = NormalizeOptions {
        rename_bindings: args.rename,
        filename: filename.clone(),
    };
    let canonical = normalize(&source, &options)?;
    let output = if args.format {
        format_cli_output(canonical, &filename, selected_formatter(true))
    } else {
        canonical
    };
    print!("{output}");
    Ok(())
}

fn run_trace(args: TraceArgs, force: bool) -> Result<()> {
    let input = fs::read_to_string(&args.input)
        .with_context(|| format!("failed to read {}", args.input.display()))?;
    let sourcemap_bytes = read_sourcemap(args.sourcemap.as_ref())?;
    let options = DecompileOptions {
        filename: args.input.to_string_lossy().to_string(),
        sourcemap: sourcemap_bytes,
        level: args.level.into(),
        ..Default::default()
    };
    let events = trace_rules(
        &input,
        options,
        RuleTraceOptions {
            start_from: args.from,
            stop_after: args.until,
            only_changed: !args.all,
        },
    )?;
    let output = format_trace_events(&events);

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UnpackInputSet {
    inputs: Vec<UnpackInput>,
    scan_stats: Option<DirectoryScanStats>,
}

fn read_unpack_inputs(inputs: &[PathBuf], heuristic_split: bool) -> Result<UnpackInputSet> {
    if inputs.is_empty() {
        let (source, filename) = read_input(None)?;
        return Ok(UnpackInputSet {
            inputs: vec![UnpackInput { filename, source }],
            scan_stats: None,
        });
    }

    let mut out = Vec::new();
    let mut saw_directory = false;
    let mut scan_stats = DirectoryScanStats::default();

    for input in inputs {
        if input == &PathBuf::from("-") || !input.is_dir() {
            let (source, filename) = read_input(Some(input))?;
            out.push(UnpackInput { filename, source });
            continue;
        }

        saw_directory = true;
        let (scanned_inputs, stats) = scan_directory_for_unpack_inputs(input, heuristic_split)?;
        scan_stats.scanned += stats.scanned;
        scan_stats.detected += stats.detected;
        scan_stats.skipped += stats.skipped;
        out.extend(scanned_inputs);
    }

    let scan_stats = if scan_stats.scanned > 0 {
        Some(scan_stats)
    } else {
        None
    };

    if saw_directory && out.is_empty() {
        bail!("no bundle or chunk files detected in directory input");
    }

    Ok(UnpackInputSet {
        inputs: out,
        scan_stats,
    })
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
