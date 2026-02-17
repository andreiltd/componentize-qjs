pub mod cli;
pub mod stubwasi;

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use stubwasi::stub_wasi_imports;
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

/// Options for componentizing a JavaScript source file.
pub struct ComponentizeOpts<'a> {
    /// Path to the WIT file or directory
    pub wit_path: &'a Path,
    /// JavaScript source code
    pub js_source: &'a str,
    /// World name to use from the WIT (None = default world)
    pub world_name: Option<&'a str>,
    /// Stub all WASI imports with traps
    pub stub_wasi: bool,
}

/// Convert JavaScript source code into a WebAssembly component.
pub async fn componentize(opts: &ComponentizeOpts<'_>) -> Result<Vec<u8>> {
    let mut resolve = Resolve::default();
    let (pkg_id, _) = resolve.push_path(opts.wit_path)?;
    let world_id = resolve.select_world(&[pkg_id], opts.world_name)?;

    let mut wit_dylib = wit_dylib::create(&resolve, world_id, None);

    wit_component::embed_component_metadata(
        &mut wit_dylib,
        &resolve,
        world_id,
        wit_component::StringEncoding::UTF8,
    )?;

    let pre_wizer_component = wit_component::Linker::default()
        .validate(true)
        .library("componentize_qjs_runtime.wasm", RUNTIME_WASM, false)?
        .library("wit-dylib.wasm", &wit_dylib, false)?
        .adapter(
            "wasi_snapshot_preview1",
            WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
        )?
        .encode()
        .context("failed to link and encode component")?;

    let mut component = wizer_init(&pre_wizer_component, opts.js_source).await?;

    if opts.stub_wasi {
        component = stub_wasi_imports(&component).context("failed to stub WASI imports")?;
    }

    Ok(component)
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
