//! Select prepared runtimes, falling back to the shared source-build recipe.

#[path = "build/runtime.rs"]
pub mod runtime;

use std::path::{Path, PathBuf};
use std::{env, fs};

use anyhow::{Context, Result, bail};
use runtime::{RUNTIME_AUDITABLE_ENV, RUNTIME_BUILDS, RuntimeOptions};

/// Track build inputs, obtain the selected artifacts, and emit embedding constants.
fn main() -> Result<()> {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR not set")?);
    let out_dir = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR not set")?);

    for input in [
        "../../Cargo.toml",
        "../../Cargo.lock",
        "../runtime/src",
        "../runtime/Cargo.toml",
        "build",
        "wit",
    ] {
        let path = manifest_dir.join(input);

        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    for build in RUNTIME_BUILDS {
        println!("cargo:rerun-if-changed=prebuilt/{}", build.filename);
    }

    for variable in [RUNTIME_AUDITABLE_ENV, "WASI_SDK_PATH", "WASM_OPT"] {
        println!("cargo:rerun-if-env-changed={variable}");
    }

    let async_support = env::var_os("CARGO_FEATURE_COMPONENT_MODEL_ASYNC").is_some();
    let prebuilt_dir = manifest_dir.join("prebuilt");

    let artifact_dir = if prebuilt_dir.join("runtime-sync.wasm").exists() {
        eprintln!("Using prebuilt runtimes from: {}", prebuilt_dir.display());
        prebuilt_dir
    } else {
        runtime::prepare(
            &manifest_dir.join("../runtime"),
            &out_dir,
            &out_dir,
            RuntimeOptions {
                release: env::var("PROFILE").is_ok_and(|profile| profile == "release"),
                async_support,
                auditable: env::var_os(RUNTIME_AUDITABLE_ENV).is_some(),
            },
        )?;
        out_dir.clone()
    };

    let source = embedding_source(&artifact_dir, async_support)?;
    fs::write(out_dir.join("output.rs"), source).context("Failed to write output.rs")
}

/// Embed every selected artifact, aliasing disabled async variants to sync ones.
fn embedding_source(artifact_dir: &Path, async_support: bool) -> Result<String> {
    let mut source = String::new();

    for build in RUNTIME_BUILDS {
        if build.async_support && !async_support {
            let fallback = build
                .fallback_const
                .expect("async variants have sync fallbacks");
            source.push_str(&format!(
                "const {}: &[u8] = {fallback};\n",
                build.const_name
            ));
            continue;
        }

        let path = artifact_dir.join(build.filename);

        if !path.is_file() {
            bail!(
                "Prepared {} runtime is missing at {}. If installing from crates.io, \
                 this is a packaging bug.",
                build.name,
                path.display(),
            );
        }

        source.push_str(&format!(
            "const {}: &[u8] = include_bytes!({path:?});\n",
            build.const_name
        ));
    }

    Ok(source)
}
