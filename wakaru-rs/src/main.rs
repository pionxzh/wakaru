use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use rayon::prelude::*;
use wakaru_rs::{
    decompile, extract_sources, format_trace_events, parse_sourcemap, trace_rules, unpack,
    unpack_raw, DecompileOptions, RewriteLevel, RuleTraceOptions,
};

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

    /// Input JavaScript/TypeScript file.
    ///
    /// Use `-` to read from stdin. If omitted and stdin is piped, stdin is read
    /// automatically.
    input: Option<PathBuf>,

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

    let (input, filename) = read_input(cli.input.as_ref())?;

    let heuristic_split = !matches!(cli.unpack, Some(UnpackMode::Strict));
    let options = DecompileOptions {
        filename,
        sourcemap_path: cli.sourcemap.map(|p| p.to_string_lossy().into_owned()),
        level: cli.level.into(),
        heuristic_split,
        ..Default::default()
    };

    if cli.unpack.is_some() {
        let out_dir = cli.output.expect("checked above");
        ensure_output_dir(&out_dir, cli.force)?;

        let pairs = if cli.raw {
            unpack_raw(&input, &options)?
        } else {
            unpack(&input, options)?
        };

        // Resolve output paths (serial — deduplication needs mutable seen set).
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let resolved: Vec<(PathBuf, &str)> = pairs
            .iter()
            .map(|(filename, code)| {
                let out_path = deduplicate_path(&out_dir.join(filename), &mut seen);
                (out_path, code.as_str())
            })
            .collect();

        // Batch-create all unique parent directories.
        let mut dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for (path, _) in &resolved {
            if let Some(parent) = path.parent() {
                if dirs.insert(parent.to_path_buf()) {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create output directory {}", parent.display())
                    })?;
                }
            }
        }

        // Write files in parallel.
        resolved.par_iter().try_for_each(|(path, code)| {
            fs::write(path, code).with_context(|| format!("failed to write {}", path.display()))
        })?;

        for (path, _) in &resolved {
            eprintln!("wrote {}", path.display());
        }
        eprintln!("total: {} module(s)", resolved.len());
    } else {
        let output = decompile(&input, options)?;

        match cli.output {
            Some(path) => {
                ensure_output_file(&path, cli.force)?;
                fs::write(&path, output)
                    .with_context(|| format!("failed to write {}", path.display()))?;
            }
            None => {
                print!("{output}");
            }
        }
    }

    Ok(())
}

fn run_extract(args: ExtractArgs, force: bool) -> Result<()> {
    let map_bytes = fs::read(&args.map)
        .with_context(|| format!("failed to read source map {}", args.map.display()))?;
    let sm = parse_sourcemap(&map_bytes)?;
    ensure_output_dir(&args.output, force)?;
    let n = extract_sources(&sm, &args.output)?;
    eprintln!("extracted {n} source file(s) to {}", args.output.display());
    Ok(())
}

fn run_debug(args: DebugArgs, force: bool) -> Result<()> {
    match args.command {
        DebugCommand::Trace(args) => run_trace(args, force),
    }
}

fn run_trace(args: TraceArgs, force: bool) -> Result<()> {
    let input = fs::read_to_string(&args.input)
        .with_context(|| format!("failed to read {}", args.input.display()))?;
    let options = DecompileOptions {
        filename: args.input.to_string_lossy().to_string(),
        sourcemap_path: args.sourcemap.map(|p| p.to_string_lossy().into_owned()),
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

fn ensure_output_file(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "output file {} already exists; pass --force to overwrite",
            path.display()
        );
    }
    Ok(())
}

fn ensure_output_dir(path: &PathBuf, force: bool) -> Result<()> {
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
    } else {
        fs::create_dir_all(path)
            .with_context(|| format!("failed to create output directory {}", path.display()))?;
    }
    Ok(())
}

/// Return a path that hasn't been used yet, disambiguating case collisions.
///
/// `seen` stores the lowercased string representation of every path already
/// claimed.  When a collision is detected the stem gets a numeric suffix:
/// `foo.js` → `foo_2.js` → `foo_3.js` …
fn deduplicate_path(path: &Path, seen: &mut std::collections::HashSet<String>) -> PathBuf {
    let key = path.to_string_lossy().to_lowercase();
    if seen.insert(key) {
        return path.to_path_buf();
    }
    // Collision — append _N before the extension.
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("js");
    let parent = path.parent().unwrap_or(std::path::Path::new("."));
    let mut n = 2u32;
    loop {
        let candidate = parent.join(format!("{stem}_{n}.{ext}"));
        let candidate_key = candidate.to_string_lossy().to_lowercase();
        if seen.insert(candidate_key) {
            return candidate;
        }
        n += 1;
    }
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
    fn parses_source_map_aliases() {
        let cli = Cli::try_parse_from(["wakaru", "input.js", "--source-map", "input.js.map"])
            .expect("--source-map should parse");
        assert_eq!(cli.sourcemap, Some(PathBuf::from("input.js.map")));

        let cli = Cli::try_parse_from(["wakaru", "input.js", "--sourcemap", "input.js.map"])
            .expect("--sourcemap alias should parse");
        assert_eq!(cli.sourcemap, Some(PathBuf::from("input.js.map")));
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
        assert!(ensure_output_dir(&dir, true).is_ok());

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("wakaru-cli-test-{name}-{nanos}"))
    }
}
