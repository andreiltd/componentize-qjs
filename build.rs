use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;

const WASI_SDK_VERSION: &str = "30";
const WASI_SKD_DL_URL: &str = "https://github.com/WebAssembly/wasi-sdk/releases/download";

const BINARYEN_VERSION: &str = "126";
const BINARYEN_DL_URL: &str = "https://github.com/WebAssembly/binaryen/releases/download";

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=crates/runtime/src/lib.rs");
    println!("cargo:rerun-if-changed=crates/runtime/Cargo.toml");

    let out_dir = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR not set")?);
    let target = "wasm32-wasip2";
    let upcase = target.to_uppercase().replace('-', "_");

    // Get wasi-sdk - from env, cached, or download
    let wasi_sdk = get_wasi_sdk(&out_dir)?;
    eprintln!("Using wasi-sdk at: {}", wasi_sdk.display());

    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let optimize_size = env::var("CARGO_FEATURE_OPTIMIZE_SIZE").is_ok();
    let is_release = profile == "release";

    // Link libc statically into the shared wasm module
    let flags = "-Clink-arg=-shared -Clink-arg=-Wl,--no-entry";
    let rustflags = match (is_release, optimize_size) {
        (true, true) => format!("{flags} -Clto=fat -Copt-level=z"),
        (true, false) => format!("{flags} -Clto=fat -Copt-level=3"),
        (_, _) => flags.to_string(),
    };

    let flags = "-fPIC";
    let cflags = match (is_release, optimize_size) {
        (true, true) => format!("{flags} -Oz"),
        (true, false) => format!("{flags} -O3"),
        (_, _) => flags.to_string(),
    };

    let clang = wasi_sdk.join("bin/clang");
    let mut cargo = Command::new("cargo");
    cargo
        .arg("build")
        .arg("--target")
        .arg(target)
        .arg("--package=componentize-qjs-runtime")
        .arg("--no-default-features")
        .env("CARGO_TARGET_DIR", &out_dir)
        .env(format!("CARGO_TARGET_{upcase}_RUSTFLAGS"), rustflags)
        .env(format!("CARGO_TARGET_{upcase}_LINKER"), &clang)
        .env(format!("CFLAGS_{}", target.replace('-', "_")), cflags)
        .env(format!("CC_{}", target.replace('-', "_")), &clang)
        .env("WASI_SDK_PATH", &wasi_sdk)
        .env("WASI_SDK", &wasi_sdk)
        .env_remove("CARGO_ENCODED_RUSTFLAGS");

    if is_release {
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

    fs::copy(&runtime_src, &runtime_dst)
        .with_context(|| format!("Failed to copy {}", runtime_src.display()))?;

    if is_release {
        let wasm_opt = get_wasm_opt(&out_dir)?;
        let opt_level = if optimize_size { "-Oz" } else { "-O3" };

        let status = Command::new(&wasm_opt)
            .arg(opt_level)
            .arg("--all-features")
            .arg("--disable-gc")
            .arg("--disable-reference-types")
            .arg("--strip-debug")
            .arg("--strip-producers")
            .arg(&runtime_dst)
            .arg("-o")
            .arg(&runtime_dst)
            .status()
            .context("Failed to run wasm-opt")?;

        if !status.success() {
            bail!("wasm-opt failed");
        }
    }

    println!(
        "cargo:rustc-env=RUNTIME_WASM_PATH={}",
        runtime_dst.display()
    );

    let output = format!(
        r#"const RUNTIME_WASM: &[u8] = include_bytes!({:?});"#,
        runtime_dst,
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

    // Download wasi-sdk
    let (arch, os) = system()?;
    let filename = format!("wasi-sdk-{WASI_SDK_VERSION}.0-{arch}-{os}.tar.gz");
    let url = format!("{WASI_SKD_DL_URL}/wasi-sdk-{WASI_SDK_VERSION}/{filename}");

    http_archive(&url, out_dir)?;

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

fn get_wasm_opt(out_dir: &Path) -> Result<PathBuf> {
    // Check WASM_OPT environment variable first
    if let Ok(path) = env::var("WASM_OPT") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
    }

    // Check cached location
    let stable = out_dir.join("binaryen");
    let wasm_opt = stable.join("bin/wasm-opt");
    if wasm_opt.exists() {
        return Ok(wasm_opt);
    }

    // Download binaryen
    let (arch, os) = system()?;
    let tag = format!("version_{BINARYEN_VERSION}");
    let filename = format!("binaryen-{tag}-{arch}-{os}.tar.gz");
    let url = format!("{BINARYEN_DL_URL}/{tag}/{filename}");

    http_archive(&url, out_dir)?;

    // Rename extracted directory to stable location
    let extracted = find_binaryen(out_dir).context("Could not find extracted binaryen")?;
    fs::rename(&extracted, &stable).context("Failed to rename binaryen directory")?;

    Ok(stable.join("bin/wasm-opt"))
}

fn find_binaryen(target_dir: &Path) -> Option<PathBuf> {
    let pattern = target_dir.join("binaryen*");
    glob::glob(pattern.to_str()?)
        .ok()?
        .filter_map(Result::ok)
        .find(|entry| entry.is_dir() && entry.join("bin/wasm-opt").exists())
}

fn system() -> Result<(&'static str, &'static str)> {
    let (arch, os) = match (env::consts::ARCH, env::consts::OS) {
        ("x86_64", "linux") => ("x86_64", "linux"),
        ("aarch64", "linux") => ("arm64", "linux"),
        ("x86_64", "macos") => ("x86_64", "macos"),
        ("aarch64", "macos") => ("arm64", "macos"),
        ("x86_64", "windows") => ("x86_64", "windows"),
        ("aarch64", "windows") => ("arm64", "windows"),
        (arch, os) => bail!("Unsupported platform: {arch}-{os}"),
    };

    Ok((arch, os))
}

fn http_archive(url: &str, out_dir: &Path) -> Result<()> {
    eprintln!("Downloading archive from {url}...");

    let response = ureq::get(url)
        .call()
        .context("Failed to download wasi-sdk")?;

    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(500_000_000)
        .read_to_end(&mut bytes)
        .context("Failed to download archive")?;

    let decoder = GzDecoder::new(bytes.as_slice());

    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(out_dir)
        .context("Failed to extract archive")?;

    Ok(())
}
