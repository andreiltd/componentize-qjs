use crate::{componentize, ComponentizeOpts};

use anyhow::{Context, Result};
use clap::Parser;

use std::fs;

#[derive(Parser)]
#[command(name = "componentize-qjs")]
#[command(about = "Convert JavaScript to WebAssembly components using QuickJS")]
pub struct CliArgs {
    /// Path to the WIT file or directory
    #[arg(short, long)]
    pub wit: std::path::PathBuf,

    /// Path to the JavaScript source file
    #[arg(short, long)]
    pub js: std::path::PathBuf,

    /// Output path for the component
    #[arg(short, long, default_value = "output.wasm")]
    pub output: std::path::PathBuf,

    /// World name to use from the WIT
    #[arg(short = 'n', long)]
    pub world: Option<String>,

    /// Stub all WASI imports with traps
    #[arg(long)]
    pub stub_wasi: bool,
}

/// Run the componentize-qjs CLI with the given arguments.
pub async fn run(args: Vec<String>) -> Result<()> {
    let args =
        CliArgs::try_parse_from(std::iter::once("componentize-qjs".to_string()).chain(args))?;

    if !args.wit.exists() {
        anyhow::bail!("WIT file/directory not found: {}", args.wit.display());
    }
    if !args.js.exists() {
        anyhow::bail!("JavaScript file not found: {}", args.js.display());
    }

    let js_source = fs::read_to_string(&args.js)
        .with_context(|| format!("failed to read JS file: {}", args.js.display()))?;

    println!("componentize-qjs");
    println!("  WIT:    {}", args.wit.display());
    println!("  JS:     {}", args.js.display());
    println!("  Output: {}", args.output.display());

    if args.stub_wasi {
        println!("Stubbing WASI imports...");
    }

    let component = componentize(&ComponentizeOpts {
        wit_path: &args.wit,
        js_source: &js_source,
        world_name: args.world.as_deref(),
        stub_wasi: args.stub_wasi,
    })
    .await?;

    fs::write(&args.output, &component)
        .with_context(|| format!("failed to write output to {}", args.output.display()))?;

    println!("Component written to {}", args.output.display());
    println!("  Size: {} bytes", component.len());

    Ok(())
}
