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

    let opts = componentize_qjs::ComponentizeOpts {
        wit_path: &wit_path,
        js_source: &opts.js_source,
        world_name: opts.world.as_deref(),
        stub_wasi: opts.stub_wasi.unwrap_or(false),
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
    match componentize_qjs::cli::run(args).await {
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
