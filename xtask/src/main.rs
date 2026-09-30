//! Prepare runtime Wasm directly, without building the host componentizer.

#[path = "../../crates/core/build/runtime.rs"]
pub mod runtime;

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use runtime::{RUNTIME_AUDITABLE_ENV, RUNTIME_BUILDS, RuntimeOptions};

/// Portable runtime-artifact preparation options.
#[derive(Parser)]
#[command(about = "Prepare componentize-qjs runtime Wasm artifacts")]
struct Args {
    /// Directory receiving the stable runtime*.wasm filenames.
    #[arg(long, default_value = "target/runtime")]
    output: PathBuf,
    /// Directory for tool downloads and disposable nested Cargo targets.
    #[arg(long, default_value = "target/runtime-build")]
    build_dir: PathBuf,
    /// Optimize the runtime with release settings and wasm-opt.
    #[arg(long)]
    release: bool,
    /// Prepare only the two non-async runtime variants.
    #[arg(long)]
    sync_only: bool,
    /// Retain dependency metadata using cargo-auditable.
    #[arg(long)]
    auditable: bool,
}

/// Build the requested runtime variants and report their explicit output paths.
fn main() -> Result<()> {
    let args = Args::parse();
    let runtime_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/runtime");
    let options = RuntimeOptions {
        release: args.release,
        async_support: !args.sync_only,
        auditable: args.auditable || std::env::var_os(RUNTIME_AUDITABLE_ENV).is_some(),
    };

    runtime::prepare(&runtime_dir, &args.output, &args.build_dir, options)?;

    for build in RUNTIME_BUILDS {
        if !build.async_support || options.async_support {
            println!("{}", args.output.join(build.filename).display());
        }
    }

    Ok(())
}
