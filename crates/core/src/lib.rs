pub mod codegen;
pub mod stubwasi;

use std::path::{Component as PathComponent, Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use bytes::Bytes;
use stubwasi::stub_wasi_imports;
use wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER;
use wasmtime::component::{Component as WasmtimeComponent, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wizer::{WasmtimeWizerComponent, Wizer};
use wit_parser::Resolve;

include!(concat!(env!("OUT_DIR"), "/output.rs"));

wasmtime::component::bindgen!({
    path: "wit/init.wit",
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
    /// Path to the JavaScript entry file, used as the base for resolving imports
    pub js_path: Option<&'a Path>,
    /// Host directory exposed read-only during Wizer for resolving imported modules
    pub module_root: Option<&'a Path>,
    /// World name to use from the WIT (None = default world)
    pub world_name: Option<&'a str>,
    /// Stub all WASI imports with traps
    pub stub_wasi: bool,
    /// Disable automatic garbage collection in the QuickJS runtime
    pub disable_gc: bool,
    /// Runtime to embed before Wizer initialization
    pub runtime: Runtime<'a>,
}

/// QuickJS runtime variant to embed in the generated component.
#[derive(Clone, Copy, Debug)]
pub enum Runtime<'a> {
    /// Standard runtime optimized for speed.
    ///
    /// Built with component-model async support when the `component-model-async`
    /// feature is enabled (the default); otherwise this is the non-async runtime.
    Default,
    /// Runtime optimized for smaller generated components.
    ///
    /// Built with component-model async support when the `component-model-async`
    /// feature is enabled (the default); otherwise this is the non-async runtime.
    OptSize,
    /// Non-async runtime optimized for speed.
    ///
    /// Produces components that do not use the component-model async ABI, so they
    /// run on hosts without async support. Always available regardless of Cargo
    /// features.
    DefaultSync,
    /// Non-async runtime optimized for smaller generated components.
    ///
    /// Produces components that do not use the component-model async ABI, so they
    /// run on hosts without async support. Always available regardless of Cargo
    /// features.
    OptSizeSync,
    /// Caller-provided runtime Wasm bytes.
    Custom(&'a [u8]),
}

impl Default for Runtime<'_> {
    fn default() -> Self {
        default_builtin_runtime()
    }
}

/// Return the built-in runtime selected by Cargo features.
pub fn default_builtin_runtime() -> Runtime<'static> {
    if cfg!(feature = "opt-size") {
        Runtime::OptSize
    } else {
        Runtime::Default
    }
}

/// Convert JavaScript source code into a WebAssembly component.
pub async fn componentize(opts: &ComponentizeOpts<'_>) -> Result<Vec<u8>> {
    let mut resolve = Resolve::default();
    let (pkg_id, _) = resolve.push_path(opts.wit_path)?;
    let world_id = resolve.select_world(&[pkg_id], opts.world_name)?;

    let shim = codegen::generate_shim(&resolve, world_id);
    let module_resolution = module_resolution(opts)?;
    let mut wit_dylib = wit_dylib::create(&resolve, world_id, None);

    wit_component::embed_component_metadata(
        &mut wit_dylib,
        &resolve,
        world_id,
        wit_component::StringEncoding::UTF8,
    )?;

    let pre_wizer_component = wit_component::Linker::default()
        .validate(true)
        .library(
            "componentize_qjs_runtime.wasm",
            runtime_wasm(opts.runtime),
            false,
        )?
        .library("wit-dylib.wasm", &wit_dylib, false)?
        .adapter(
            "wasi_snapshot_preview1",
            WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
        )?
        .encode()
        .context("failed to link and encode component")?;

    let mut component = wizer_init(
        &pre_wizer_component,
        &shim,
        opts.js_source,
        module_resolution.as_ref(),
        opts.disable_gc,
    )
    .await?;

    if opts.stub_wasi {
        component = stub_wasi_imports(&component).context("failed to stub WASI imports")?;
    }

    Ok(component)
}

/// Return the built-in default runtime Wasm bytes.
pub fn default_runtime_wasm() -> &'static [u8] {
    DEFAULT_RUNTIME_WASM
}

/// Return the built-in opt-size runtime Wasm bytes.
pub fn opt_size_runtime_wasm() -> &'static [u8] {
    OPT_SIZE_RUNTIME_WASM
}

/// Return the built-in non-async runtime Wasm bytes.
pub fn default_sync_runtime_wasm() -> &'static [u8] {
    DEFAULT_SYNC_RUNTIME_WASM
}

/// Return the built-in non-async opt-size runtime Wasm bytes.
pub fn opt_size_sync_runtime_wasm() -> &'static [u8] {
    OPT_SIZE_SYNC_RUNTIME_WASM
}

fn runtime_wasm(runtime: Runtime<'_>) -> &[u8] {
    match runtime {
        Runtime::Default => DEFAULT_RUNTIME_WASM,
        Runtime::OptSize => OPT_SIZE_RUNTIME_WASM,
        Runtime::DefaultSync => DEFAULT_SYNC_RUNTIME_WASM,
        Runtime::OptSizeSync => OPT_SIZE_SYNC_RUNTIME_WASM,
        Runtime::Custom(wasm) => wasm,
    }
}

struct ModuleResolution {
    host_root: PathBuf,
    guest_entry_path: String,
}

fn module_resolution(opts: &ComponentizeOpts<'_>) -> Result<Option<ModuleResolution>> {
    let Some(js_path) = opts.js_path else {
        if opts.module_root.is_some() {
            return Err(anyhow!("module_root requires js_path"));
        }
        return Ok(None);
    };

    let host_entry = js_path
        .canonicalize()
        .with_context(|| format!("failed to resolve JS entry path {}", js_path.display()))?;
    if !host_entry.is_file() {
        return Err(anyhow!(
            "JS entry path is not a file: {}",
            host_entry.display()
        ));
    }

    let host_root = match opts.module_root {
        Some(root) => root
            .canonicalize()
            .with_context(|| format!("failed to resolve module root {}", root.display()))?,
        None => default_module_root(&host_entry)?,
    };
    if !host_root.is_dir() {
        return Err(anyhow!(
            "module root is not a directory: {}",
            host_root.display()
        ));
    }

    let relative_entry = host_entry.strip_prefix(&host_root).with_context(|| {
        format!(
            "JS entry path {} is not under module root {}",
            host_entry.display(),
            host_root.display()
        )
    })?;
    let guest_entry_path = guest_absolute_path(relative_entry)?;

    Ok(Some(ModuleResolution {
        host_root,
        guest_entry_path,
    }))
}

fn default_module_root(host_entry: &Path) -> Result<PathBuf> {
    let cwd = std::env::current_dir()
        .context("failed to read current directory")?
        .canonicalize()
        .context("failed to resolve current directory")?;

    if host_entry.starts_with(&cwd) {
        return Ok(cwd);
    }

    host_entry
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("JS entry path has no parent: {}", host_entry.display()))
}

fn guest_absolute_path(relative: &Path) -> Result<String> {
    let mut guest = String::from("/");
    let mut first = true;

    for component in relative.components() {
        let PathComponent::Normal(part) = component else {
            return Err(anyhow!(
                "JS entry path contains unsupported component: {}",
                relative.display()
            ));
        };
        let part = part.to_str().ok_or_else(|| {
            anyhow!(
                "JS entry path contains non-UTF-8 component: {}",
                relative.display()
            )
        })?;

        if !first {
            guest.push('/');
        }
        guest.push_str(part);
        first = false;
    }

    if first {
        return Err(anyhow!("JS entry path cannot be the module root"));
    }

    Ok(guest)
}

async fn wizer_init(
    component: &[u8],
    shim: &str,
    js: &str,
    module_resolution: Option<&ModuleResolution>,
    disable_gc: bool,
) -> Result<Vec<u8>> {
    let stdout = MemoryOutputPipe::new(10000);
    let stderr = MemoryOutputPipe::new(10000);

    let mut wasi = WasiCtxBuilder::new();
    wasi.stdin(MemoryInputPipe::new(Bytes::new()))
        .stdout(stdout.clone())
        .stderr(stderr.clone());
    if let Some(resolution) = module_resolution {
        wasi.preopened_dir(&resolution.host_root, "/", DirPerms::READ, FilePerms::READ)
            .map_err(|err| {
                anyhow!(
                    "failed to preopen module root {}: {err}",
                    resolution.host_root.display()
                )
            })?;
    }
    let wasi = wasi.build();

    let table = ResourceTable::new();
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);

    let engine = Engine::new(&config)?;
    let mut store = Store::new(&engine, Ctx { wasi, table });

    let wizer = Wizer::new();
    let (cx, instrumented) = wizer.instrument_component(component)?;
    let comp = WasmtimeComponent::new(&engine, &instrumented)?;

    let mut linker = Linker::new(&engine);
    linker.allow_shadowing(true);
    linker.define_unknown_imports_as_traps(&comp)?;
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    wasmtime_wasi::p3::add_to_linker(&mut linker)?;
    let instance = linker.instantiate_async(&mut store, &comp).await?;

    let init = Init::new(&mut store, &instance)?;
    init.call_init(
        &mut store,
        shim,
        js,
        module_resolution.map(|resolution| resolution.guest_entry_path.as_str()),
        disable_gc,
    )
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
