use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use wakaru_rs::{decompile, extract_sources, parse_sourcemap, unpack, DecompileOptions};

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
    };

    if cli.unpack {
        let pairs = unpack(&input, options)?;

        let out_dir = cli.output.unwrap_or_else(|| PathBuf::from("unpacked"));
        fs::create_dir_all(&out_dir)
            .with_context(|| format!("failed to create output directory {}", out_dir.display()))?;

        for (filename, code) in &pairs {
            let out_path = out_dir.join(filename);
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
