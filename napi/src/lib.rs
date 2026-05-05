use std::path::PathBuf;

use clap::error::ErrorKind;
use napi::bindgen_prelude::*;
use napi_derive::napi;

/// Options for componentizing a JavaScript source into a WebAssembly component.
#[napi(object)]
pub struct ComponentizeOpts {
    /// Path to the WIT file or directory
    pub wit_path: String,
    /// JavaScript source code
    pub js_source: String,
    /// World name to use from the WIT (omit for default world)
    pub world: Option<String>,
    /// Stub all WASI imports with traps (default: false)
    pub stub_wasi: Option<bool>,
    /// Disable automatic garbage collection (default: false)
    pub disable_gc: Option<bool>,
    /// Use the built-in runtime optimized for smaller generated components
    pub opt_size: Option<bool>,
    /// Path to a custom QuickJS runtime Wasm module
    pub runtime: Option<String>,
    /// Custom QuickJS runtime Wasm bytes
    pub runtime_bytes: Option<Buffer>,
}

/// Result of componentizing a JavaScript source.
#[napi(object)]
pub struct ComponentizeResult {
    /// The WebAssembly component bytes
    pub component: Buffer,
}

/// Convert JavaScript source code into a WebAssembly component.
///
/// Takes a WIT definition and JavaScript source, compiles them into a
/// WebAssembly component using the QuickJS runtime.
#[napi]
pub async fn componentize(opts: ComponentizeOpts) -> Result<ComponentizeResult> {
    let wit_path = PathBuf::from(&opts.wit_path);

    if !wit_path.exists() {
        return Err(Error::new(
            Status::InvalidArg,
            format!("WIT file/directory not found: {}", opts.wit_path),
        ));
    }

    let runtime_sources = [
        opts.opt_size.unwrap_or(false),
        opts.runtime.is_some(),
        opts.runtime_bytes.is_some(),
    ]
    .into_iter()
    .filter(|provided| *provided)
    .count();
    if runtime_sources > 1 {
        return Err(Error::new(
            Status::InvalidArg,
            "Use only one of optSize, runtime, or runtimeBytes".to_string(),
        ));
    }

    let custom_runtime = match &opts.runtime {
        Some(runtime_file) => {
            let runtime_path = PathBuf::from(runtime_file);
            if !runtime_path.exists() {
                return Err(Error::new(
                    Status::InvalidArg,
                    format!("Runtime file not found: {runtime_file}"),
                ));
            }
            Some(std::fs::read(&runtime_path).map_err(|e| {
                Error::new(
                    Status::GenericFailure,
                    format!("Failed to read runtime file {runtime_file}: {e}"),
                )
            })?)
        }
        None => opts.runtime_bytes.as_ref().map(|bytes| bytes.to_vec()),
    };
    let runtime = match custom_runtime.as_deref() {
        Some(wasm) => componentize_qjs::Runtime::Custom(wasm),
        None if opts.opt_size.unwrap_or(false) => componentize_qjs::Runtime::OptSize,
        None => componentize_qjs::Runtime::default(),
    };

    let opts = componentize_qjs::ComponentizeOpts {
        wit_path: &wit_path,
        js_source: &opts.js_source,
        world_name: opts.world.as_deref(),
        stub_wasi: opts.stub_wasi.unwrap_or(false),
        disable_gc: opts.disable_gc.unwrap_or(false),
        runtime,
    };

    let component = componentize_qjs::componentize(&opts)
        .await
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e:#}")))?;

    Ok(ComponentizeResult {
        component: component.into(),
    })
}

/// NAPI CLI entry point.
///
/// Returns `true` if the command succeeded, `false` otherwise.
#[napi]
pub async fn run_cli(args: Vec<String>) -> Result<bool> {
    match componentize_qjs_cli::cli::run(args).await {
        Ok(()) => Ok(true),
        Err(e) => {
            if let Some(clap_err) = e.downcast_ref::<clap::Error>() {
                print!("{clap_err}");
                return Ok(matches!(
                    clap_err.kind(),
                    ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
                ));
            }
            eprintln!("Error: {e:#}");
            Ok(false)
        }
    }
}
