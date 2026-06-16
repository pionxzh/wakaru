use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};

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

mod discovery;
mod formatter;
mod output;

use discovery::{scan_directory_for_unpack_inputs, DirectoryScanStats};
use formatter::{format_cli_output, selected_formatter};
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

        print_warnings(&output.warnings);
        let has_errors = output.has_errors();

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
            // Resolve output paths (serial — deduplication needs mutable seen set).
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
            // Write files in parallel.
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

        if io::stderr().is_terminal() {
            if let Some(stats) = scan_stats {
                eprintln!(
                    "scanned: {} file(s), detected: {} bundle/chunk file(s), skipped: {} file(s)",
                    stats.scanned, stats.detected, stats.skipped
                );
            }
            eprintln!("total: {} module(s)", resolved.len());
        }

        if has_errors {
            bail!("diagnostics reported errors");
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
        let output = decompile(&input, options)?;

        print_warnings(&output.warnings);
        let has_errors = output.has_errors();
        let code = format_cli_output(output.code, &output_filename, formatter);

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

        if has_errors {
            bail!("diagnostics reported errors");
        }
    }

    Ok(())
}

fn print_warnings(warnings: &[wakaru_core::UnpackWarning]) {
    for warning in warnings {
        let label = if warning.kind.is_error() {
            "error"
        } else {
            "warning"
        };
        eprintln!("{label}: {warning}");
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
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_extract_without_js_input() {
        let cli = Cli::try_parse_from(["wakaru", "extract", "input.js.map", "-o", "src"])
            .expect("extract command should parse");

        match cli.command {
            Some(Command::Extract(args)) => {
                assert_eq!(args.map, PathBuf::from("input.js.map"));
                assert_eq!(args.output, PathBuf::from("src"));
            }
            other => panic!("expected extract command, got {other:?}"),
        }
    }

    #[test]
    fn rejects_legacy_extract_flag() {
        assert!(Cli::try_parse_from([
            "wakaru",
            "input.js",
            "--extract",
            "-m",
            "input.js.map",
            "-o",
            "src"
        ])
        .is_err());
    }

    #[test]
    fn parses_debug_trace_command() {
        let cli = Cli::try_parse_from([
            "wakaru",
            "debug",
            "trace",
            "input.js",
            "--from",
            "UnEsm",
            "--until",
            "SmartInline",
            "--all",
        ])
        .expect("debug trace command should parse");

        match cli.command {
            Some(Command::Debug(DebugArgs {
                command: DebugCommand::Trace(args),
            })) => {
                assert_eq!(args.input, PathBuf::from("input.js"));
                assert_eq!(args.from.as_deref(), Some("UnEsm"));
                assert_eq!(args.until.as_deref(), Some("SmartInline"));
                assert!(args.all);
            }
            other => panic!("expected debug trace command, got {other:?}"),
        }
    }

    #[test]
    fn parses_debug_normalize_command() {
        let cli = Cli::try_parse_from(["wakaru", "debug", "normalize", "input.js", "--rename"])
            .expect("debug normalize command should parse");

        match cli.command {
            Some(Command::Debug(DebugArgs {
                command: DebugCommand::Normalize(args),
            })) => {
                assert_eq!(args.input, Some(PathBuf::from("input.js")));
                assert!(args.rename);
                assert!(!args.format);
            }
            other => panic!("expected debug normalize command, got {other:?}"),
        }
    }

    #[test]
    fn normalize_rename_then_format_canonicalizes_mangling() {
        // Mirrors run_normalize's pipeline: alpha-rename via core, then format.
        let opts = NormalizeOptions {
            rename_bindings: true,
            filename: "input.js".to_string(),
        };
        let fmt = |code: String| format_cli_output(code, "input.js", selected_formatter(true));
        let original = fmt(normalize("function load(app_id){return get(app_id)}", &opts).unwrap());
        let mangled = fmt(normalize("function l(e){return get(e)}", &opts).unwrap());
        assert_eq!(
            original, mangled,
            "mangled output should normalize identically"
        );
        assert!(original.contains("get"), "global preserved: {original}");
    }

    #[test]
    fn parses_formatter_option() {
        let cli = Cli::try_parse_from(["wakaru", "input.js", "--formatter"])
            .expect("formatter option should parse");
        assert!(cli.formatter);
    }

    #[test]
    fn parses_formatter_with_raw_unpack() {
        let cli = Cli::try_parse_from([
            "wakaru",
            "bundle.js",
            "--unpack",
            "--raw",
            "--formatter",
            "-o",
            "out",
        ])
        .expect("formatter with raw should parse");

        assert!(cli.raw);
        assert!(cli.formatter);
    }

    #[test]
    fn parses_multiple_unpack_inputs() {
        let cli = Cli::try_parse_from([
            "wakaru",
            "--unpack",
            "-o",
            "out",
            "bundle.js",
            "src_greet_js.bundle.js",
        ])
        .expect("multi-file unpack should parse");

        assert!(cli.unpack.is_some());
        assert_eq!(
            cli.inputs,
            vec![
                PathBuf::from("bundle.js"),
                PathBuf::from("src_greet_js.bundle.js")
            ]
        );
    }

    #[test]
    fn parses_profile_flag() {
        let cli = Cli::try_parse_from(["wakaru", "input.js", "--profile", "profile.json"])
            .expect("--profile should parse");
        assert_eq!(cli.profile, Some(PathBuf::from("profile.json")));
        assert!(!cli.profile_rules);
    }

    #[test]
    fn parses_profile_rules_flag() {
        let cli = Cli::try_parse_from([
            "wakaru",
            "input.js",
            "--profile",
            "profile.json",
            "--profile-rules",
        ])
        .expect("--profile-rules should parse with --profile");
        assert!(cli.profile_rules);
    }

    #[test]
    fn rejects_profile_rules_without_profile() {
        assert!(Cli::try_parse_from(["wakaru", "input.js", "--profile-rules"]).is_err());
    }

    #[test]
    fn parses_source_map_aliases() {
        let cli = Cli::try_parse_from(["wakaru", "input.js", "--source-map", "input.js.map"])
            .expect("--source-map should parse");
        assert_eq!(cli.sourcemap, Some(PathBuf::from("input.js.map")));

        let cli = Cli::try_parse_from(["wakaru", "input.js", "--sourcemap", "input.js.map"])
            .expect("--sourcemap alias should parse");
        assert_eq!(cli.sourcemap, Some(PathBuf::from("input.js.map")));
    }

    #[test]
    fn decompile_rejects_directory_input() {
        let dir = temp_test_dir("decompile-dir");
        fs::create_dir_all(&dir).expect("create temp dir");

        let cli = Cli::try_parse_from(["wakaru", dir.to_str().expect("temp path should be utf8")])
            .expect("directory input should parse");
        let err = run_default(cli).expect_err("decompile should reject directory input");
        assert!(
            err.to_string()
                .contains("cannot decompile a directory. Pass a JavaScript file or use --unpack"),
            "unexpected error: {err}"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_directory_inputs_are_recursive_detected_js_files_only() {
        let dir = temp_test_dir("unpack-dir");
        let nested = dir.join("nested");
        let hidden = dir.join(".hidden");
        let node_modules = dir.join("node_modules");
        fs::create_dir_all(&nested).expect("create nested dir");
        fs::create_dir_all(&hidden).expect("create hidden dir");
        fs::create_dir_all(&node_modules).expect("create node_modules dir");

        fs::write(dir.join("plain.js"), "const value = 1;").expect("write plain file");
        fs::write(dir.join("runtime-like.js"), runtime_like_plain_source())
            .expect("write runtime-like plain file");
        fs::write(nested.join("chunk.js"), webpack5_chunk_source()).expect("write chunk");
        fs::write(dir.join("runtime.js"), webpack5_runtime_entry_source())
            .expect("write runtime entry");
        fs::write(hidden.join("hidden.js"), webpack5_chunk_source()).expect("write hidden chunk");
        fs::write(node_modules.join("vendor.js"), webpack5_chunk_source())
            .expect("write node_modules chunk");
        fs::write(dir.join("chunk.js.map"), webpack5_chunk_source()).expect("write sourcemap");

        let input_set =
            read_unpack_inputs(std::slice::from_ref(&dir), false).expect("read directory inputs");
        assert_eq!(
            input_set.scan_stats,
            Some(DirectoryScanStats {
                scanned: 4,
                detected: 2,
                skipped: 2,
            })
        );
        assert_eq!(
            input_set.inputs.len(),
            2,
            "expected visible chunk and runtime entry"
        );
        assert!(
            input_set
                .inputs
                .iter()
                .any(|input| input.filename.ends_with("nested\\chunk.js")
                    || input.filename.ends_with("nested/chunk.js")),
            "missing detected chunk input: {:?}",
            input_set.inputs
        );
        assert!(
            input_set
                .inputs
                .iter()
                .any(|input| input.filename.ends_with("runtime.js")),
            "missing detected runtime input: {:?}",
            input_set.inputs
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_directory_without_detected_files_errors() {
        let dir = temp_test_dir("unpack-dir-empty");
        fs::create_dir_all(&dir).expect("create temp dir");
        fs::write(dir.join("plain.js"), "const value = 1;").expect("write plain file");

        let err = read_unpack_inputs(std::slice::from_ref(&dir), false)
            .expect_err("directory with no detected bundles should error");
        assert!(
            err.to_string()
                .contains("no bundle or chunk files detected in directory input"),
            "unexpected error: {err}"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn output_file_requires_force_to_overwrite() {
        let dir = temp_test_dir("output-file");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("out.js");
        fs::write(&path, "old").expect("write temp file");

        assert!(ensure_output_file(&path, false).is_err());
        assert!(ensure_output_file(&path, true).is_ok());

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn output_dir_requires_force_when_non_empty() {
        let dir = temp_test_dir("output-dir");
        fs::create_dir_all(&dir).expect("create temp dir");
        fs::write(dir.join("entry.js"), "old").expect("write temp file");

        assert!(ensure_output_dir(&dir, false).is_err());
        assert!(ensure_output_dir(&dir, true).expect("force should allow non-empty dir"));

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_cli_does_not_write_overlapping_dot_payload_outside_output_dir() {
        let dir = temp_test_dir("unpack-cli-overlap");
        let out_dir = dir.join("out");
        let bundle_path = dir.join("bundle.js");
        let outside_target = dir.join("node_modules/@wakaru/cli/bin/wakaru");
        fs::create_dir_all(outside_target.parent().expect("outside target parent"))
            .expect("create outside target parent");
        fs::write(&outside_target, "original").expect("write outside marker");
        fs::write(&bundle_path, overlapping_dot_webpack5_bundle()).expect("write bundle");

        let cli = Cli::try_parse_from([
            "wakaru",
            bundle_path.to_str().expect("bundle path should be utf8"),
            "--unpack",
            "-o",
            out_dir.to_str().expect("output path should be utf8"),
        ])
        .expect("cli should parse");
        run_default(cli).expect("unpack should succeed");

        assert_eq!(
            fs::read_to_string(&outside_target).expect("read outside marker"),
            "original",
            "outside marker must not be overwritten"
        );
        assert!(
            out_dir
                .join("..../node_modules/@wakaru/cli/bin/wakaru")
                .exists(),
            "payload should be written under the output directory"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn output_dir_reports_when_existing_writes_need_checks() {
        let empty_dir = temp_test_dir("output-dir-empty");
        fs::create_dir_all(&empty_dir).expect("create temp dir");
        assert!(
            !ensure_output_dir(&empty_dir, false).expect("empty dir should be accepted"),
            "empty directories can write directly without checking existing files"
        );
        fs::remove_dir_all(&empty_dir).expect("remove empty temp dir");

        let new_dir = temp_test_dir("output-dir-new");
        assert!(
            !ensure_output_dir(&new_dir, false).expect("new dir should be created"),
            "new directories can write directly without checking existing files"
        );
        fs::remove_dir_all(&new_dir).expect("remove new temp dir");

        let non_empty_dir = temp_test_dir("output-dir-non-empty");
        fs::create_dir_all(&non_empty_dir).expect("create temp dir");
        fs::write(non_empty_dir.join("entry.js"), "old").expect("write temp file");
        assert!(
            ensure_output_dir(&non_empty_dir, true).expect("force should allow non-empty dir"),
            "non-empty forced directories should preserve write-if-changed checks"
        );
        fs::remove_dir_all(&non_empty_dir).expect("remove non-empty temp dir");
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("wakaru-cli-test-{name}-{nanos}"))
    }

    fn webpack5_chunk_source() -> &'static str {
        r#"
(self.webpackChunk = self.webpackChunk || []).push([
  [1],
  {
    100: function(module, exports, require) {
      "use strict";
      require.r(exports);
      exports.default = 1;
    }
  }
]);
"#
    }

    fn webpack5_runtime_entry_source() -> &'static str {
        r#"
(() => {
  var modules = {};
  function require(id) { return {}; }
  require.m = modules;
  require.f = {};
  require.e = function(id) { return Promise.resolve(id); };
  require.u = function(id) { return id + ".bundle.js"; };
  require.t = function(value) { return value; };
  require.e(529).then(require.t.bind(require, 529, 19));
})();
"#
    }

    fn runtime_like_plain_source() -> &'static str {
        r#"
(() => {
  const api = {};
  api.e = 1;
  api.u = 2;
  api.t = 3;
  api.m = 4;
})();
"#
    }

    fn overlapping_dot_webpack5_bundle() -> &'static str {
        r#"
(() => {
  var __webpack_modules__ = ({
    "....//node_modules/@wakaru/cli/bin/wakaru": ((module) => {
      module.exports = "pwned";
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  console.log(__webpack_require__("....//node_modules/@wakaru/cli/bin/wakaru"));
})();
"#
    }
}
