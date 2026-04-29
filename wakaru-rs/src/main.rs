use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use wakaru_rs::{
    decompile, extract_sources, format_trace_events, parse_sourcemap, trace_rules, unpack,
    unpack_raw, DecompileOptions, RuleTraceOptions,
};

#[derive(Debug, Parser)]
#[command(name = "wakaru-rs")]
#[command(about = "Rust rewrite of Wakaru's unminify core")]
struct Cli {
    /// Input JavaScript/TypeScript file.
    input: PathBuf,

    /// Output file (or output directory when --unpack is set). Prints to stdout when omitted.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Unpack a webpack bundle into individual module files.
    /// When set, --output is treated as the output directory.
    #[arg(short, long)]
    unpack: bool,

    /// With --unpack, write raw unpacker output before the decompiler rule pipeline.
    #[arg(long, requires = "unpack")]
    raw: bool,

    /// Optional source map file (.map) for enhanced decompilation:
    /// - Deduplicates identical imports collapsed by the bundler
    /// - Recovers original identifier names using source map position data
    #[arg(short = 'm', long, value_name = "FILE")]
    sourcemap: Option<PathBuf>,

    /// Extract the original source files embedded in the source map's
    /// `sourcesContent` and write them to the output directory.
    /// Requires --sourcemap.  The input JS file is not decompiled.
    #[arg(long, requires = "sourcemap")]
    extract: bool,

    /// Trace the single-file rule pipeline and print per-rule before/after output.
    /// Bundle inputs are not supported by this mode yet.
    #[arg(long, conflicts_with_all = ["unpack", "extract"])]
    trace_rules: bool,

    /// Include rules that ran but did not change the rendered output.
    #[arg(long, requires = "trace_rules")]
    trace_all: bool,

    /// First rule to run when tracing.
    #[arg(long, value_name = "RULE", requires = "trace_rules")]
    trace_from: Option<String>,

    /// Last rule to run when tracing.
    #[arg(long, value_name = "RULE", requires = "trace_rules")]
    trace_until: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // --extract: write original source files from sourcesContent to disk.
    if cli.extract {
        let map_path = cli.sourcemap.as_ref().unwrap(); // guaranteed by clap `requires`
        let map_bytes = fs::read(map_path)
            .with_context(|| format!("failed to read source map {}", map_path.display()))?;
        let sm = parse_sourcemap(&map_bytes)?;
        let out_dir = cli.output.unwrap_or_else(|| PathBuf::from("extracted"));
        let n = extract_sources(&sm, &out_dir)?;
        eprintln!("extracted {n} source file(s) to {}", out_dir.display());
        return Ok(());
    }

    let input = fs::read_to_string(&cli.input)
        .with_context(|| format!("failed to read {}", cli.input.display()))?;

    let options = DecompileOptions {
        filename: cli.input.to_string_lossy().to_string(),
        sourcemap_path: cli.sourcemap.map(|p| p.to_string_lossy().into_owned()),
        ..Default::default()
    };

    if cli.trace_rules {
        let events = trace_rules(
            &input,
            options,
            RuleTraceOptions {
                start_from: cli.trace_from,
                stop_after: cli.trace_until,
                only_changed: !cli.trace_all,
            },
        )?;
        let output = format_trace_events(&events);

        match cli.output {
            Some(path) => {
                fs::write(&path, output)
                    .with_context(|| format!("failed to write {}", path.display()))?;
            }
            None => {
                print!("{output}");
            }
        }
    } else if cli.unpack {
        let pairs = if cli.raw {
            unpack_raw(&input)?
        } else {
            unpack(&input, options)?
        };

        let out_dir = cli.output.unwrap_or_else(|| PathBuf::from("unpacked"));
        fs::create_dir_all(&out_dir)
            .with_context(|| format!("failed to create output directory {}", out_dir.display()))?;

        // Track seen paths in a case-folded set so we detect collisions on
        // case-insensitive filesystems (Windows NTFS).  When a collision is
        // found we append `_2`, `_3`, … before the extension.
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (filename, code) in &pairs {
            let out_path = deduplicate_path(&out_dir.join(filename), &mut seen);
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create output directory {}", parent.display())
                })?;
            }
            fs::write(&out_path, code)
                .with_context(|| format!("failed to write {}", out_path.display()))?;
            eprintln!("wrote {}", out_path.display());
        }
        eprintln!("total: {} module(s)", pairs.len());
    } else {
        let output = decompile(&input, options)?;

        match cli.output {
            Some(path) => {
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

/// Return a path that hasn't been used yet, disambiguating case collisions.
///
/// `seen` stores the lowercased string representation of every path already
/// claimed.  When a collision is detected the stem gets a numeric suffix:
/// `foo.js` → `foo_2.js` → `foo_3.js` …
fn deduplicate_path(
    path: &PathBuf,
    seen: &mut std::collections::HashSet<String>,
) -> PathBuf {
    let key = path.to_string_lossy().to_lowercase();
    if seen.insert(key) {
        return path.clone();
    }
    // Collision — append _N before the extension.
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("module");
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
