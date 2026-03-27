use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use wakaru_rs::{decompile, unpack, DecompileOptions};

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let input = fs::read_to_string(&cli.input)
        .with_context(|| format!("failed to read {}", cli.input.display()))?;

    if cli.unpack {
        let pairs = unpack(
            &input,
            DecompileOptions {
                filename: cli.input.to_string_lossy().to_string(),
            },
        )?;

        let out_dir = cli.output.unwrap_or_else(|| PathBuf::from("unpacked"));
        fs::create_dir_all(&out_dir)
            .with_context(|| format!("failed to create output directory {}", out_dir.display()))?;

        for (filename, code) in &pairs {
            let out_path = out_dir.join(filename);
            fs::write(&out_path, code)
                .with_context(|| format!("failed to write {}", out_path.display()))?;
            eprintln!("wrote {}", out_path.display());
        }
        eprintln!("total: {} module(s)", pairs.len());
    } else {
        let output = decompile(
            &input,
            DecompileOptions {
                filename: cli.input.to_string_lossy().to_string(),
            },
        )?;

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
