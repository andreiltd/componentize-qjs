use std::io::Read;
use std::mem;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use wasm_encoder::{ComponentSectionId, Encode, RawSection, Section};
use wasmparser::{Parser, Payload::*};

const WASI_SDK_VERSION: &str = "30";
const WASI_SKD_DL_URL: &str = "https://github.com/WebAssembly/wasi-sdk/releases/download";

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=crates/runtime/src/lib.rs");
    println!("cargo:rerun-if-changed=crates/runtime/Cargo.toml");

    let out_dir = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR not set")?);
    let target = "wasm32-wasip2";
    let upcase = target.to_uppercase().replace('-', "_");

    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());

    // Get wasi-sdk - from env, cached, or download
    let wasi_sdk = get_wasi_sdk(&out_dir)?;
    eprintln!("Using wasi-sdk at: {}", wasi_sdk.display());

    let rustflags = "-Clink-arg=-shared -Clink-self-contained=n";
    let mut cargo = Command::new("cargo");
    cargo
        .arg("build")
        .arg("--target")
        .arg(target)
        .arg("--package=componentize-qjs-runtime")
        .env("CARGO_TARGET_DIR", &out_dir)
        .env(format!("CARGO_TARGET_{upcase}_RUSTFLAGS"), rustflags)
        .env(
            format!("CARGO_TARGET_{upcase}_LINKER"),
            wasi_sdk.join("bin/clang"),
        )
        .env(
            format!("CC_{}", target.replace('-', "_")),
            wasi_sdk.join("bin/clang"),
        )
        .env(format!("CFLAGS_{}", target.replace('-', "_")), "-fPIC")
        .env("WASI_SDK_PATH", &wasi_sdk)
        .env("WASI_SDK", &wasi_sdk)
        .env_remove("CARGO_ENCODED_RUSTFLAGS");

    if profile == "release" {
        cargo.arg("--release");
    }

    eprintln!("Building runtime: {cargo:?}");
    let status = cargo.status().context("Failed to run cargo build")?;
    if !status.success() {
        bail!("Failed to build runtime");
    }

    let runtime_src = out_dir
        .join(target)
        .join(&profile)
        .join("componentize_qjs_runtime.wasm");

    let runtime_dst = out_dir.join("runtime.wasm");

    let bytes = fs::read(&runtime_src)
        .with_context(|| format!("Failed to read {}", runtime_src.display()))?;

    let stripped_runtime = strip_wasm(&bytes);
    fs::write(&runtime_dst, stripped_runtime).context("Failed to write runtime.wasm")?;

    println!(
        "cargo:rustc-env=RUNTIME_WASM_PATH={}",
        runtime_dst.display()
    );

    // Copy and strip wasi-sdk shared libraries
    let sysroot_lib = wasi_sdk.join("share/wasi-sysroot/lib").join(target);
    let libs = ["libc.so"];

    for lib in libs {
        let src = sysroot_lib.join(lib);
        if !src.exists() {
            bail!("{lib} not found at: {}", src.display());
        }

        let bytes = fs::read(&src).with_context(|| format!("Failed to read {lib}"))?;
        let stripped = strip_wasm(&bytes);
        fs::write(out_dir.join(lib), stripped).with_context(|| format!("Failed to write {lib}"))?;
    }

    let output = format!(
        r#"const RUNTIME_WASM: &[u8] = include_bytes!({:?});
           const LIBC_SO: &[u8] = include_bytes!({:?});
        "#,
        runtime_dst,
        out_dir.join("libc.so"),
    );

    fs::write(out_dir.join("output.rs"), output).context("Failed to write output.rs")?;
    Ok(())
}

fn get_wasi_sdk(out_dir: &Path) -> Result<PathBuf> {
    // Check environment first
    if let Ok(path) = env::var("WASI_SDK_PATH") {
        let p = PathBuf::from(path);
        if p.join("bin/clang").exists() {
            return Ok(p);
        }
    }

    // Check cached location
    let stable = out_dir.join("wasi-sdk");
    if stable.join("bin/clang").exists() {
        return Ok(stable);
    }

    // Download wasi-sdk - determine platform
    let (arch, os) = match (env::consts::ARCH, env::consts::OS) {
        ("x86_64", "linux") => ("x86_64", "linux"),
        ("aarch64", "linux") => ("arm64", "linux"),
        ("x86_64", "macos") => ("x86_64", "macos"),
        ("aarch64", "macos") => ("arm64", "macos"),
        ("x86_64", "windows") => ("x86_64", "windows"),
        ("aarch64", "windows") => ("arm64", "windows"),
        (arch, os) => bail!("Unsupported platform: {arch}-{os}"),
    };

    let filename = format!("wasi-sdk-{WASI_SDK_VERSION}.0-{arch}-{os}.tar.gz");
    let url = format!("{WASI_SKD_DL_URL}/wasi-sdk-{WASI_SDK_VERSION}/{filename}");

    eprintln!("Downloading wasi-sdk from {url}...");

    let response = ureq::get(&url)
        .call()
        .context("Failed to download wasi-sdk")?;

    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(500_000_000) // 500MB limit
        .read_to_end(&mut bytes)
        .context("Failed to read wasi-sdk archive")?;

    eprintln!("Extracting wasi-sdk...");

    let decoder = GzDecoder::new(bytes.as_slice());
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(out_dir)
        .context("Failed to extract wasi-sdk")?;

    // Rename extracted directory to stable location
    let extracted = find_wasi_sdk(out_dir).context("Could not find extracted wasi-sdk")?;
    fs::rename(&extracted, &stable).context("Failed to rename wasi-sdk directory")?;

    Ok(stable)
}

fn find_wasi_sdk(target_dir: &Path) -> Option<PathBuf> {
    let pattern = target_dir.join("wasi-sdk*");
    glob::glob(pattern.to_str()?)
        .ok()?
        .filter_map(Result::ok)
        .find(|entry| entry.is_dir() && entry.join("bin/clang").exists())
}

/// It looks like wasi-sdk only provides unstripped libc so we strip debug/custom
/// sections from a wasm module manually to reduce size. Keeps: name, component-type:*,
/// dylink.0. This is adapted from `wasm-tools strip`
fn strip_wasm(input: &[u8]) -> Vec<u8> {
    let strip_custom_section =
        |name: &str| name != "name" && !name.starts_with("component-type:") && name != "dylink.0";

    let mut output = Vec::new();
    let mut stack = Vec::new();

    for payload in Parser::new(0).parse_all(input) {
        let payload = match payload {
            Ok(p) => p,
            Err(_) => return input.to_vec(),
        };

        match payload {
            Version { encoding, .. } => {
                output.extend_from_slice(match encoding {
                    wasmparser::Encoding::Component => &wasm_encoder::Component::HEADER,
                    wasmparser::Encoding::Module => &wasm_encoder::Module::HEADER,
                });
            }
            ModuleSection { .. } | ComponentSection { .. } => {
                stack.push(mem::take(&mut output));
                continue;
            }
            End { .. } => {
                let mut parent = match stack.pop() {
                    Some(c) => c,
                    None => break,
                };
                if output.starts_with(&wasm_encoder::Component::HEADER) {
                    parent.push(ComponentSectionId::Component as u8);
                    output.encode(&mut parent);
                } else {
                    parent.push(ComponentSectionId::CoreModule as u8);
                    output.encode(&mut parent);
                }
                output = parent;
            }
            _ => {}
        }

        if let CustomSection(c) = &payload {
            if strip_custom_section(c.name()) {
                continue;
            }
        }

        if let Some((id, range)) = payload.as_section() {
            RawSection {
                id,
                data: &input[range],
            }
            .append_to(&mut output);
        }
    }

    output
}
