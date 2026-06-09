use std::collections::HashMap;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use rayon::prelude::*;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;
use wakaru_core::{
    decompile, extract_source_entries, format_trace_events, parse_sourcemap, trace_rules, unpack,
    unpack_files, unpack_files_raw, unpack_raw, DecompileOptions, RewriteLevel, RuleTraceOptions,
    UnpackInput,
};

mod formatter;

use formatter::{format_cli_output, selected_formatter};

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

    /// With --unpack, write a provenance.json in the output directory mapping
    /// each module file to the byte ranges in the original input it was
    /// extracted from.
    #[arg(long, requires = "unpack")]
    provenance: bool,

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
        let options = DecompileOptions {
            filename,
            sourcemap: sourcemap_bytes,
            level: cli.level.into(),
            heuristic_split,
            diagnostics: cli.diagnostics,
            ..Default::default()
        };

        let out_dir = cli.output.expect("checked above");
        let check_existing_writes = ensure_output_dir(&out_dir, cli.force)?;
        let out_dir = canonicalize_output_dir(&out_dir)?;

        // Provenance entries from single-source unpacks leave `input` empty;
        // remember the input name so the provenance file can attribute them.
        let single_input_name = (inputs.len() == 1).then(|| inputs[0].filename.clone());

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

        let provenance = output.provenance;
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

        if cli.provenance {
            // Map each module to its final on-disk relative path: `resolved`
            // is parallel to `pairs`, and CLI-side dedup may have renamed.
            let final_names: HashMap<&str, String> = pairs
                .iter()
                .zip(resolved.iter())
                .map(|((filename, _), (path, _))| {
                    let relative = path
                        .strip_prefix(&out_dir)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    (filename.as_str(), relative)
                })
                .collect();
            let json = render_provenance_json(
                &provenance,
                &final_names,
                single_input_name.as_deref().unwrap_or(""),
            );
            let provenance_path = out_dir.join("provenance.json");
            fs::write(&provenance_path, json)
                .with_context(|| format!("failed to write {}", provenance_path.display()))?;
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
        let options = DecompileOptions {
            filename,
            sourcemap: sourcemap_bytes,
            level: cli.level.into(),
            heuristic_split,
            diagnostics: cli.diagnostics,
            ..Default::default()
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
    }
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DirectoryScanStats {
    scanned: usize,
    detected: usize,
    skipped: usize,
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
        for path in collect_directory_js_inputs(input)? {
            scan_stats.scanned += 1;
            let source = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            if is_detected_unpack_input(&source, heuristic_split) {
                scan_stats.detected += 1;
                out.push(UnpackInput {
                    filename: path.to_string_lossy().to_string(),
                    source,
                });
            } else {
                scan_stats.skipped += 1;
            }
        }
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

fn collect_directory_js_inputs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    collect_directory_js_inputs_inner(root, &mut paths)?;
    paths.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    Ok(paths)
}

fn collect_directory_js_inputs_inner(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("failed to read input directory {}", dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to read input directory {}", dir.display()))?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;

        if file_type.is_dir() {
            if is_hidden_name(&file_name) || file_name == "node_modules" {
                continue;
            }
            collect_directory_js_inputs_inner(&path, paths)?;
        } else if file_type.is_file() && !is_hidden_name(&file_name) && is_js_like_input(&path) {
            paths.push(path);
        }
    }

    Ok(())
}

fn is_detected_unpack_input(source: &str, heuristic_split: bool) -> bool {
    matches!(
        wakaru_core::unpacker::try_unpack_bundle(source),
        Ok(Some(_))
    ) || (heuristic_split
        && wakaru_core::scope_hoist::split_scope_hoisted(source)
            .is_some_and(|result| result.modules.len() > 1))
}

fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
}

fn is_js_like_input(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "js" | "mjs" | "cjs"))
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

fn write_file(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

fn write_if_changed(path: &Path, content: &str) -> Result<()> {
    if let Ok(metadata) = fs::metadata(path) {
        if metadata.len() == content.len() as u64
            && fs::read(path).is_ok_and(|existing| existing == content.as_bytes())
        {
            return Ok(());
        }
    }

    write_file(path, content)
}

fn canonicalize_output_dir(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("failed to canonicalize output directory {}", path.display()))
}

fn resolve_unpack_output_path(
    out_dir: &Path,
    filename: &str,
    seen: &mut std::collections::HashSet<String>,
) -> Result<PathBuf> {
    let relative = safe_relative_module_path(filename)?;
    let lexical_path = deduplicate_path(&out_dir.join(relative), seen);
    canonicalize_unpack_output_path(out_dir, &lexical_path, filename)
}

fn safe_relative_module_path(filename: &str) -> Result<PathBuf> {
    let mut relative = PathBuf::new();
    for component in Path::new(filename).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("unsafe module filename {filename:?}: path escapes output directory")
            }
        }
    }

    if relative.as_os_str().is_empty() {
        bail!("unsafe module filename {filename:?}: empty output path");
    }

    Ok(relative)
}

fn canonicalize_unpack_output_path(
    out_dir: &Path,
    lexical_path: &Path,
    filename: &str,
) -> Result<PathBuf> {
    let relative = lexical_path.strip_prefix(out_dir).with_context(|| {
        format!("unsafe module filename {filename:?}: path escapes output directory")
    })?;
    let Some(file_name) = relative.file_name() else {
        bail!("unsafe module filename {filename:?}: empty output path");
    };

    let parent_relative = relative.parent().unwrap_or_else(|| Path::new(""));
    let mut current = out_dir.to_path_buf();
    for component in parent_relative.components() {
        let Component::Normal(part) = component else {
            bail!("unsafe module filename {filename:?}: path escapes output directory");
        };
        current.push(part);
        if current.exists() {
            let canonical = current.canonicalize().with_context(|| {
                format!(
                    "failed to canonicalize output directory {}",
                    current.display()
                )
            })?;
            ensure_path_inside_output_dir(out_dir, &canonical, filename)?;
            if !canonical.is_dir() {
                bail!(
                    "output path {} exists and is not a directory",
                    current.display()
                );
            }
            current = canonical;
        } else {
            fs::create_dir(&current).with_context(|| {
                format!("failed to create output directory {}", current.display())
            })?;
            let canonical = current.canonicalize().with_context(|| {
                format!(
                    "failed to canonicalize output directory {}",
                    current.display()
                )
            })?;
            ensure_path_inside_output_dir(out_dir, &canonical, filename)?;
            current = canonical;
        }
    }

    let candidate = current.join(file_name);
    let target = if candidate.exists() {
        candidate.canonicalize().with_context(|| {
            format!("failed to canonicalize output file {}", candidate.display())
        })?
    } else {
        candidate
    };
    ensure_path_inside_output_dir(out_dir, &target, filename)?;
    Ok(target)
}

fn ensure_path_inside_output_dir(out_dir: &Path, path: &Path, filename: &str) -> Result<()> {
    if path.starts_with(out_dir) {
        Ok(())
    } else {
        bail!("unsafe module filename {filename:?}: path escapes output directory");
    }
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

/// Render provenance entries as a JSON document.
///
/// `final_names` maps the driver's module filename to the relative path the
/// CLI actually wrote (CLI-side dedup can rename). `default_input` fills in
/// entries whose input is empty (single-source unpacks).
fn render_provenance_json(
    provenance: &[wakaru_core::ModuleProvenance],
    final_names: &HashMap<&str, String>,
    default_input: &str,
) -> String {
    let mut entries: Vec<(String, &wakaru_core::ModuleProvenance)> = provenance
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

    let mut json = String::from("{\n  \"modules\": {\n");
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
        json.push_str(&format!(
            "    \"{}\": {{\"input\": \"{}\", \"ranges\": [{}]}}{}\n",
            json_escape(name),
            json_escape(input),
            ranges,
            if i + 1 < entries.len() { "," } else { "" }
        ));
    }
    json.push_str("  }\n}\n");
    json
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
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn renders_provenance_json_with_final_names_and_default_input() {
        let provenance = vec![
            wakaru_core::ModuleProvenance {
                filename: "b.js".to_string(),
                input: String::new(),
                ranges: vec![(10, 20), (30, 40)],
            },
            wakaru_core::ModuleProvenance {
                filename: "a \"quoted\".js".to_string(),
                input: "chunk-1.js".to_string(),
                ranges: vec![(0, 5)],
            },
        ];
        let mut final_names = HashMap::new();
        // CLI-side dedup renamed b.js on disk.
        final_names.insert("b.js", "b_2.js".to_string());

        let json = render_provenance_json(&provenance, &final_names, "bundle.js");

        assert!(
            json.contains(r#""b_2.js": {"input": "bundle.js", "ranges": [[10,20],[30,40]]}"#),
            "renamed module with default input missing:\n{json}"
        );
        assert!(
            json.contains(r#""a \"quoted\".js": {"input": "chunk-1.js", "ranges": [[0,5]]}"#),
            "escaped filename with explicit input missing:\n{json}"
        );
        // Must be alphabetically sorted and valid JSON shape.
        assert!(json.find("a \\\"quoted\\\"").unwrap() < json.find("b_2.js").unwrap());
        assert!(json.starts_with("{\n  \"modules\": {\n"));
        assert!(json.ends_with("  }\n}\n"));
    }

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
    fn unpack_output_path_rejects_parent_dir_components() {
        let dir = temp_test_dir("unpack-output-escape");
        fs::create_dir_all(&dir).expect("create temp dir");
        let out_dir = canonicalize_output_dir(&dir).expect("canonicalize output dir");
        let mut seen = std::collections::HashSet::new();

        let err = resolve_unpack_output_path(
            &out_dir,
            "../node_modules/@wakaru/cli/bin/wakaru",
            &mut seen,
        )
        .expect_err("parent path should be rejected");
        assert!(
            err.to_string().contains("path escapes output directory"),
            "unexpected error: {err}"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_keeps_overlap_payload_inside_output_dir() {
        let dir = temp_test_dir("unpack-output-overlap");
        fs::create_dir_all(&dir).expect("create temp dir");
        let out_dir = canonicalize_output_dir(&dir).expect("canonicalize output dir");
        let mut seen = std::collections::HashSet::new();

        let path = resolve_unpack_output_path(
            &out_dir,
            "....//node_modules/@wakaru/cli/bin/wakaru",
            &mut seen,
        )
        .expect("overlapping dots are an ordinary relative directory");
        assert!(
            path.starts_with(&out_dir),
            "resolved path should stay in output dir: {}",
            path.display()
        );
        assert!(path.ends_with("node_modules/@wakaru/cli/bin/wakaru"));

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_rejects_absolute_paths() {
        let dir = temp_test_dir("unpack-output-absolute");
        fs::create_dir_all(&dir).expect("create temp dir");
        let out_dir = canonicalize_output_dir(&dir).expect("canonicalize output dir");
        let mut seen = std::collections::HashSet::new();
        let absolute = format!(
            "{}tmp{}escape.js",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        );

        let err = resolve_unpack_output_path(&out_dir, &absolute, &mut seen)
            .expect_err("absolute module path should be rejected");
        assert!(
            err.to_string().contains("path escapes output directory"),
            "unexpected error: {err}"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[cfg(windows)]
    #[test]
    fn unpack_output_path_rejects_windows_drive_prefixes() {
        let dir = temp_test_dir("unpack-output-drive-prefix");
        fs::create_dir_all(&dir).expect("create temp dir");
        let out_dir = canonicalize_output_dir(&dir).expect("canonicalize output dir");
        let mut seen = std::collections::HashSet::new();

        let err = resolve_unpack_output_path(&out_dir, r"C:\tmp\escape.js", &mut seen)
            .expect_err("drive-prefixed module path should be rejected");
        assert!(
            err.to_string().contains("path escapes output directory"),
            "unexpected error: {err}"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_rejects_parent_directory_that_is_file() {
        let dir = temp_test_dir("unpack-output-file-parent");
        fs::create_dir_all(&dir).expect("create temp dir");
        fs::write(dir.join("src"), "not a directory").expect("write file parent");
        let out_dir = canonicalize_output_dir(&dir).expect("canonicalize output dir");
        let mut seen = std::collections::HashSet::new();

        let err = resolve_unpack_output_path(&out_dir, "src/index.js", &mut seen)
            .expect_err("file parent should be rejected");
        assert!(
            err.to_string().contains("exists and is not a directory"),
            "unexpected error: {err}"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_deduplicates_after_safety_checks() {
        let dir = temp_test_dir("unpack-output-dedup");
        fs::create_dir_all(&dir).expect("create temp dir");
        let out_dir = canonicalize_output_dir(&dir).expect("canonicalize output dir");
        let mut seen = std::collections::HashSet::new();

        let first = resolve_unpack_output_path(&out_dir, "src/index.js", &mut seen)
            .expect("first path should resolve");
        let second = resolve_unpack_output_path(&out_dir, "src/index.js", &mut seen)
            .expect("second path should resolve with suffix");

        assert!(first.starts_with(&out_dir), "{}", first.display());
        assert!(second.starts_with(&out_dir), "{}", second.display());
        assert_ne!(first, second);
        assert_eq!(first.file_name().and_then(|s| s.to_str()), Some("index.js"));
        assert_eq!(
            second.file_name().and_then(|s| s.to_str()),
            Some("index_2.js")
        );

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
    fn unpack_output_path_rejects_symlink_parent_that_points_outside() {
        let dir = temp_test_dir("unpack-output-symlink-parent");
        let out_dir_raw = dir.join("out");
        let external_dir = dir.join("external");
        fs::create_dir_all(&out_dir_raw).expect("create output dir");
        fs::create_dir_all(&external_dir).expect("create external dir");
        let link_path = out_dir_raw.join("link");
        if create_dir_symlink(&external_dir, &link_path).is_err() {
            fs::remove_dir_all(&dir).expect("remove temp dir");
            return;
        }
        let out_dir = canonicalize_output_dir(&out_dir_raw).expect("canonicalize output dir");
        let mut seen = std::collections::HashSet::new();

        let err = resolve_unpack_output_path(&out_dir, "link/pwn.js", &mut seen)
            .expect_err("symlink parent escaping output dir should be rejected");
        assert!(
            err.to_string().contains("path escapes output directory"),
            "unexpected error: {err}"
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

    #[test]
    fn write_if_changed_skips_identical_readonly_file() {
        let dir = temp_test_dir("write-if-changed");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("entry.js");
        fs::write(&path, "same").expect("write temp file");

        let original_permissions = fs::metadata(&path).expect("metadata").permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).expect("set readonly");

        assert!(write_if_changed(&path, "same").is_ok());

        fs::set_permissions(&path, original_permissions).expect("restore permissions");
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn write_if_changed_overwrites_different_length_file() {
        let dir = temp_test_dir("write-if-changed-length");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("entry.js");
        fs::write(&path, "short").expect("write temp file");

        write_if_changed(&path, "longer content").expect("write changed file");

        assert_eq!(
            fs::read_to_string(&path).expect("read updated file"),
            "longer content"
        );
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("wakaru-cli-test-{name}-{nanos}"))
    }

    #[cfg(windows)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[cfg(unix)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
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
