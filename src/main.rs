mod stubwasi;
use stubwasi::stub_wasi_imports;

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use clap::Parser;
use wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wizer::{WasmtimeWizerComponent, Wizer};
use wit_parser::Resolve;

include!(concat!(env!("OUT_DIR"), "/output.rs"));

wasmtime::component::bindgen!({
    path: "crates/runtime/wit/init.wit",
    world: "init",
    exports: { default: async },
});

#[derive(Parser)]
#[command(name = "componentize-qjs")]
#[command(about = "Convert JavaScript to WebAssembly components using QuickJS")]
struct Args {
    /// Path to the WIT file or directory
    #[arg(short, long)]
    wit: PathBuf,

    /// Path to the JavaScript source file
    #[arg(short, long)]
    js: PathBuf,

    /// Output path for the component
    #[arg(short, long, default_value = "output.wasm")]
    output: PathBuf,

    /// World name to use from the WIT
    #[arg(short = 'n', long)]
    world: Option<String>,

    /// Stub all WASI imports with traps
    #[arg(long)]
    stub_wasi: bool,
}

struct Ctx {
    wasi: WasiCtx,
    table: ResourceTable,
}

impl WasiView for Ctx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Verify inputs exist
    if !args.wit.exists() {
        anyhow::bail!("WIT file/directory not found: {}", args.wit.display());
    }
    if !args.js.exists() {
        anyhow::bail!("JavaScript file not found: {}", args.js.display());
    }

    // Parse the WIT file(s)
    let mut resolve = Resolve::default();
    let (pkg_id, _) = resolve.push_path(&args.wit)?;
    let world_id = resolve.select_world(&[pkg_id], args.world.as_deref())?;
    let world_name = &resolve.worlds[world_id].name;

    println!("componentize-qjs");
    println!("  WIT:    {}", args.wit.display());
    println!("  World:  {}", world_name);
    println!("  JS:     {}", args.js.display());
    println!("  Output: {}", args.output.display());

    // Generate the wit-dylib adapter
    let mut wit_dylib = wit_dylib::create(&resolve, world_id, None);

    // Embed component metadata (tells Linker what WIT world this implements)
    wit_component::embed_component_metadata(
        &mut wit_dylib,
        &resolve,
        world_id,
        wit_component::StringEncoding::UTF8,
    )?;

    let js_source = fs::read_to_string(&args.js)
        .with_context(|| format!("failed to read JS file: {}", args.js.display()))?;

    // Link the component with shared libraries
    let pre_wizer_component = wit_component::Linker::default()
        .validate(true)
        .library("libc.so", LIBC_SO, false)?
        .library("libsetjmp.so", LIBSETJMP_SO, false)?
        .library(
            "libwasi-emulated-signal.so",
            LIBWASI_EMULATED_SIGNAL_SO,
            false,
        )?
        .library("componentize_qjs_runtime.wasm", RUNTIME_WASM, false)?
        .library("wit-dylib.wasm", &wit_dylib, false)?
        .adapter(
            "wasi_snapshot_preview1",
            WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
        )?
        .encode()
        .context("failed to link and encode component")?;

    // Use Wizer to pre-initialize with JS source
    // Then restore _initialize exports that Wizer strips
    let mut component = wizer_init(&pre_wizer_component, &js_source).await?;

    // Optionally stub WASI imports so the component is self-contained
    if args.stub_wasi {
        println!("Stubbing WASI imports...");
        component = stub_wasi_imports(&component).context("failed to stub WASI imports")?;
    }

    // Write the output
    fs::write(&args.output, &component)
        .with_context(|| format!("failed to write output to {}", args.output.display()))?;

    println!("Component written to {}", args.output.display());
    println!("  Size: {} bytes", component.len());

    Ok(())
}

async fn wizer_init(component: &[u8], js: &str) -> Result<Vec<u8>> {
    let stdout = MemoryOutputPipe::new(10000);
    let stderr = MemoryOutputPipe::new(10000);

    let mut wasi = WasiCtxBuilder::new();
    let wasi = wasi
        .stdin(MemoryInputPipe::new(Bytes::new()))
        .stdout(stdout.clone())
        .stderr(stderr.clone())
        .build();

    let table = ResourceTable::new();
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);

    let engine = Engine::new(&config)?;
    let mut store = Store::new(&engine, Ctx { wasi, table });

    let wizer = Wizer::new();
    let (cx, instrumented) = wizer.instrument_component(component)?;
    let comp = Component::new(&engine, &instrumented)?;

    let mut linker = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    let instance = linker.instantiate_async(&mut store, &comp).await?;

    // Call the init function exported by the runtime
    let init = Init::new(&mut store, &instance)?;
    init.call_init(&mut store, js)
        .await?
        .map_err(|e| anyhow!("{e}"))
        .with_context(move || {
            format!(
                "{}{}",
                String::from_utf8_lossy(&stdout.contents()),
                String::from_utf8_lossy(&stderr.contents())
            )
        })?;

    // Snapshot the component state
    let component = wizer
        .snapshot_component(
            cx,
            &mut WasmtimeWizerComponent {
                store: &mut store,
                instance,
            },
        )
        .await?;

    Ok(component)
}
