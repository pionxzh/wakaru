use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use wakaru_rs::{decompile, DecompileOptions};

#[derive(Debug, Parser)]
#[command(name = "wakaru-rs")]
#[command(about = "Rust rewrite of Wakaru's unminify core")]
struct Cli {
    /// Input JavaScript/TypeScript file.
    input: PathBuf,

    /// Output file. Prints to stdout when omitted.
    #[arg(short, long)]
    output: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let input = fs::read_to_string(&cli.input)
        .with_context(|| format!("failed to read {}", cli.input.display()))?;

    let output = decompile(
        &input,
        DecompileOptions {
            filename: cli.input.to_string_lossy().to_string(),
        },
    )?;

    match cli.output {
        Some(path) => {
            fs::write(&path, output).with_context(|| format!("failed to write {}", path.display()))?;
        }
        None => {
            print!("{output}");
        }
    }

    Ok(())
}
