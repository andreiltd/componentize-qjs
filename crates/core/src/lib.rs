//! Link JavaScript, WIT bindings, and a QuickJS runtime into a WebAssembly component.
//!
//! Componentization builds a temporary component, evaluates the generated shim
//! and user module under Wizer, then snapshots the initialized runtime.

pub mod codegen;
mod resolver;
pub mod stubwasi;

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use bytes::Bytes;
use resolver::Resolver;
use stubwasi::{stub_internal_imports, stub_wasi_imports};
use wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER;
use wasmtime::component::types::ComponentItem;
use wasmtime::component::{
    Component as WasmtimeComponent, Linker, LinkerInstance, ResourceTable, ResourceType,
};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
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

impl Runtime<'static> {
    /// Select a built-in runtime from the CLI-style sync and size options.
    pub fn builtin(sync: bool, opt_size: bool) -> Self {
        match (sync, opt_size) {
            (true, true) => Self::OptSizeSync,
            (true, false) => Self::DefaultSync,
            (false, true) => Self::OptSize,
            (false, false) => default_builtin_runtime(),
        }
    }
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

    let resolver = module_resolution(opts)?;
    let (mut wit_dylib, wit_metadata) = wit_dylib::create_with_metadata(&resolve, world_id, None);
    let shim = codegen::generate_shim(&resolve, &wit_metadata);

    wit_component::embed_component_metadata(
        &mut wit_dylib,
        &resolve,
        world_id,
        wit_component::StringEncoding::UTF8,
    )?;

    let mut linker = wit_component::Linker::default();
    linker.encoder().validate(true);
    linker.library(
        "componentize_qjs_runtime.wasm",
        runtime_wasm(opts.runtime),
        false,
    )?;
    linker.library("wit-dylib.wasm", &wit_dylib, false)?;
    linker.encoder().adapter(
        "wasi_snapshot_preview1",
        WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
    )?;

    let pre_wizer_component = linker
        .encode()
        .context("failed to link and encode component")?;

    let mut component = wizer_init(
        &pre_wizer_component,
        &shim,
        opts.js_source,
        resolver,
        opts.disable_gc,
    )
    .await?;

    component = stub_internal_imports(&component)
        .context("failed to stub internal module-loader import")?;

    if opts.stub_wasi {
        component = stub_wasi_imports(&component).context("failed to stub WASI imports")?;
    }

    Ok(component)
}

fn module_resolution(opts: &ComponentizeOpts<'_>) -> Result<Option<Resolver>> {
    let Some(js_path) = opts.js_path else {
        if opts.module_root.is_some() {
            return Err(anyhow!("module_root requires js_path"));
        }
        return Ok(None);
    };

    Resolver::new(js_path, opts.module_root).map(Some)
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

/// Supply placeholders so Wizer can instantiate the component and evaluate JS.
/// Real WASI and module-loader definitions replace these placeholders afterward;
/// any remaining host function traps if called during initialization.
fn define_unknown_imports_as_traps(
    linker: &mut Linker<Ctx>,
    engine: &Engine,
    component: &WasmtimeComponent,
) -> Result<()> {
    let mut resources = Vec::new();
    for (name, item) in component.component_type().imports(engine) {
        trap_import(
            &mut linker.root(),
            engine,
            name,
            name,
            item.ty,
            &mut resources,
        )?;
    }
    Ok(())
}

/// Stub one import while preserving the types Wasmtime checks during linking.
/// `resources` is shared across namespaces to recognize aliases of earlier imports.
fn trap_import(
    linker: &mut LinkerInstance<'_, Ctx>,
    engine: &Engine,
    name: &str,
    path: &str,
    ty: ComponentItem,
    resources: &mut Vec<ResourceType>,
) -> Result<()> {
    match ty {
        ComponentItem::ComponentInstance(instance) => {
            // Imported interfaces are namespaces whose members also need stubs.
            let mut nested = linker.instance(name)?;
            for (export, item) in instance.exports(engine) {
                trap_import(
                    &mut nested,
                    engine,
                    export,
                    &format!("{path}#{export}"),
                    item.ty,
                    resources,
                )?;
            }
        }
        ComponentItem::ComponentFunc(func) => {
            let message = format!("unknown import: `{path}` has not been defined");
            // Even a trapping function must match the import's sync/async ABI.
            if func.async_() {
                linker.func_new_concurrent(name, move |_, _, _, _| {
                    let message = message.clone();
                    Box::pin(async move { Err(wasmtime::Error::msg(message)) })
                })?;
            } else {
                linker.func_new(name, move |_, _, _, _| {
                    Err(wasmtime::Error::msg(message.clone()))
                })?;
            }
        }
        ComponentItem::Resource(ty) => {
            // Give unrelated resources distinct identities. Leave equality aliases
            // absent so Wasmtime follows the original, including later WASI overrides.
            if !resources.contains(&ty) {
                let resource = ResourceType::host_dynamic(u32::try_from(resources.len())?);
                resources.push(ty);
                // Trapping constructors cannot create values needing destruction.
                linker.resource(name, resource, |_, _| Ok(()))?;
            }
        }
        ComponentItem::Component(_) | ComponentItem::Module(_) => {
            anyhow::bail!("unable to define component or module import `{path}` as a trap");
        }
        _ => {}
    }
    Ok(())
}

async fn wizer_init(
    component: &[u8],
    shim: &str,
    js: &str,
    resolver: Option<Resolver>,
    disable_gc: bool,
) -> Result<Vec<u8>> {
    let stdout = MemoryOutputPipe::new(10000);
    let stderr = MemoryOutputPipe::new(10000);

    let wasi = WasiCtxBuilder::new()
        .stdin(MemoryInputPipe::new(Bytes::new()))
        .stdout(stdout.clone())
        .stderr(stderr.clone())
        .build();

    let table = ResourceTable::new();
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.wasm_component_model_map(true);

    let engine = Engine::new(&config)?;
    let mut store = Store::new(&engine, Ctx { wasi, table });

    let wizer = Wizer::new();
    let (cx, instrumented) = wizer.instrument_component(component)?;
    let comp = WasmtimeComponent::new(&engine, &instrumented)?;

    let mut linker = Linker::new(&engine);
    linker.allow_shadowing(true);
    define_unknown_imports_as_traps(&mut linker, &engine, &comp)?;
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    wasmtime_wasi::p3::add_to_linker(&mut linker)?;

    register_module_loader(&mut linker, resolver.clone())?;

    let instance = linker.instantiate_async(&mut store, &comp).await?;
    let init = Init::new(&mut store, &instance)?;
    init.call_init(
        &mut store,
        shim,
        js,
        resolver.as_ref().map(Resolver::entry_path),
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

fn register_module_loader(linker: &mut Linker<Ctx>, resolver: Option<Resolver>) -> Result<()> {
    let resolve = resolver.clone();
    let load = resolver;

    let mut instance = linker.instance("local:init/module-loader")?;
    instance.func_wrap(
        "resolve",
        move |_, (referrer, specifier): (String, String)| -> wasmtime::Result<_> {
            let result = resolve.as_ref().map_or_else(
                || {
                    Err(
                        "filesystem module not found: module resolution requires js_path"
                            .to_string(),
                    )
                },
                |resolver| {
                    resolver
                        .resolve(&referrer, &specifier)
                        .map_err(|err| err.to_string())
                },
            );
            Ok((result,))
        },
    )?;
    instance.func_wrap(
        "load",
        move |_, (path,): (String,)| -> wasmtime::Result<_> {
            let result = load.as_ref().map_or_else(
                || Err("filesystem module not found: module loading requires js_path".to_string()),
                |resolver| resolver.load(&path).map_err(|err| err.to_string()),
            );
            Ok((result,))
        },
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_stubs_preserve_resource_identity_and_async_types() {
        let mut config = Config::new();
        config.wasm_component_model_async(true);
        let engine = Engine::new(&config).unwrap();
        let component = WasmtimeComponent::new(
            &engine,
            r#"(component
                (import "original" (type $original (sub resource)))
                (import "unrelated" (type (sub resource)))
                (import "alias" (type (eq $original)))
                (type $resources (instance
                    (export "input-stream" (type (sub resource)))
                ))
                (import "one" (instance (type $resources)))
                (import "two" (instance (type $resources)))
                (type $async (func async))
                (import "root" (func (type $async)))
                (import "nested" (instance
                    (export "inner" (instance
                        (type $async (func async))
                        (export "call" (func (type $async)))
                    ))
                ))
            )"#,
        )
        .unwrap();
        let mut linker = Linker::<Ctx>::new(&engine);
        linker.allow_shadowing(true);
        define_unknown_imports_as_traps(&mut linker, &engine, &component).unwrap();
        let resource = |ty: &wasmtime::component::types::Component, name| {
            let ComponentItem::Resource(resource) = ty.get_import(&engine, name).unwrap().ty else {
                panic!("expected resource");
            };
            resource
        };
        let stubs = linker.substituted_component_type(&component).unwrap();
        assert_ne!(resource(&stubs, "original"), resource(&stubs, "unrelated"));
        assert_eq!(resource(&stubs, "original"), resource(&stubs, "alias"));
        let instance_resource = |name| {
            let ComponentItem::ComponentInstance(instance) =
                stubs.get_import(&engine, name).unwrap().ty
            else {
                panic!("expected instance");
            };
            let ComponentItem::Resource(resource) =
                instance.get_export(&engine, "input-stream").unwrap().ty
            else {
                panic!("expected resource");
            };
            resource
        };
        assert_ne!(instance_resource("one"), instance_resource("two"));
        // Real host definitions replace stubs after discovery, as WASI does.
        let host = ResourceType::host::<wasmtime_wasi::p2::DynInputStream>();
        linker
            .root()
            .resource("original", host, |_, _| Ok(()))
            .unwrap();
        let ty = linker.substituted_component_type(&component).unwrap();
        assert_eq!(resource(&ty, "original"), host);
        assert_eq!(resource(&ty, "alias"), host);
        assert_ne!(resource(&ty, "unrelated"), host);
    }
}
