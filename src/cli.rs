use componentize_qjs::{ComponentizeOpts, Runtime, componentize};

use anyhow::{Context, Result};
use clap::Parser;
use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_minifier::{
    CompressOptions, CompressOptionsKeepNames, CompressOptionsUnused, MangleOptions, Minifier,
    MinifierOptions,
};
use oxc_parser::Parser as OxcParser;
use oxc_span::SourceType;

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

    /// Root directory exposed during Wizer for resolving JavaScript imports
    #[arg(long, value_name = "PATH")]
    pub module_root: Option<std::path::PathBuf>,

    /// Output path for the component
    #[arg(short, long, default_value = "output.wasm")]
    pub output: std::path::PathBuf,

    /// World name to use from the WIT
    #[arg(short = 'n', long)]
    pub world: Option<String>,

    /// Stub all WASI imports with traps
    #[arg(long)]
    pub stub_wasi: bool,

    /// Minify the JS source via oxc before componentizing
    #[arg(short = 'm', long)]
    pub minify: bool,

    /// Disable automatic garbage collection in the QuickJS runtime
    #[arg(long)]
    pub disable_gc: bool,

    /// Use the built-in runtime optimized for smaller generated components
    #[arg(long, conflicts_with = "runtime")]
    pub opt_size: bool,

    /// Use the built-in non-async runtime, producing components that do not use
    /// the component-model async ABI
    #[arg(long, conflicts_with = "runtime")]
    pub sync: bool,

    /// Path to a custom QuickJS runtime Wasm module
    #[arg(long, value_name = "PATH")]
    pub runtime: Option<std::path::PathBuf>,
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
    if let Some(runtime_file) = &args.runtime
        && !runtime_file.exists()
    {
        anyhow::bail!("Runtime file not found: {}", runtime_file.display());
    }
    if let Some(module_root) = &args.module_root
        && !module_root.exists()
    {
        anyhow::bail!("Module root not found: {}", module_root.display());
    }

    let js_source = fs::read_to_string(&args.js)
        .with_context(|| format!("failed to read JS file: {}", args.js.display()))?;

    let js_source = if args.minify {
        let allocator = Allocator::default();
        let source_type = SourceType::mjs();
        let ret = OxcParser::new(&allocator, &js_source, source_type).parse();
        let mut program = ret.program;

        let options = MinifierOptions {
            mangle: Some(MangleOptions {
                top_level: Some(false),
                ..Default::default()
            }),
            compress: Some(CompressOptions {
                unused: CompressOptionsUnused::Keep,
                keep_names: CompressOptionsKeepNames::all_false(),
                ..CompressOptions::default()
            }),
        };
        let ret = Minifier::new(options).minify(&allocator, &mut program);
        Codegen::new()
            .with_scoping(ret.scoping)
            .build(&program)
            .code
    } else {
        js_source
    };

    println!("componentize-qjs");
    println!("  WIT:    {}", args.wit.display());
    println!("  JS:     {}", args.js.display());
    println!("  Output: {}", args.output.display());

    let runtime = match &args.runtime {
        Some(file) => Runtime::Custom(&fs::read(file)?),
        None => match (args.sync, args.opt_size) {
            (true, true) => Runtime::OptSizeSync,
            (true, false) => Runtime::DefaultSync,
            (false, true) => Runtime::OptSize,
            (false, false) => Runtime::default(),
        },
    };

    if args.stub_wasi {
        println!("Stubbing WASI imports...");
    }

    let component = componentize(&ComponentizeOpts {
        wit_path: &args.wit,
        js_source: &js_source,
        js_path: Some(&args.js),
        module_root: args.module_root.as_deref(),
        world_name: args.world.as_deref(),
        stub_wasi: args.stub_wasi,
        disable_gc: args.disable_gc,
        runtime,
    })
    .await?;

    fs::write(&args.output, &component)
        .with_context(|| format!("failed to write output to {}", args.output.display()))?;

    println!("Component written to {}", args.output.display());
    println!("  Size: {} bytes", component.len());

    Ok(())
}
