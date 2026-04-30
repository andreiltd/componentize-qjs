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
const RUNTIME_AUDITABLE_ENV: &str = "COMPONENTIZE_QJS_RUNTIME_AUDITABLE";
const MAX_ARCHIVE_BYTES: u64 = 1_000_000_000;

fn main() -> Result<()> {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR not set")?);
    let runtime_dir = manifest_dir.join("../runtime");

    println!("cargo:rerun-if-changed={}/src", runtime_dir.display());
    println!(
        "cargo:rerun-if-changed={}/Cargo.toml",
        runtime_dir.display()
    );
    println!("cargo:rerun-if-changed=prebuilt/runtime.wasm");
    println!("cargo:rerun-if-env-changed={RUNTIME_AUDITABLE_ENV}");

    let out_dir = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR not set")?);

    // Check for pre-built runtime (used when installing from crates.io)
    let prebuilt = manifest_dir.join("prebuilt/runtime.wasm");
    if prebuilt.exists() {
        eprintln!("Using pre-built runtime at: {}", prebuilt.display());
        return emit_runtime_wasm(&prebuilt, &out_dir);
    }

    // Check that runtime source is available (won't be when installed from crates.io
    // without a pre-built runtime)
    let runtime_src_dir = runtime_dir.join("src");
    if !runtime_src_dir.exists() {
        bail!(
            "Runtime source not found at {} and no pre-built runtime at {}. \
             If installing from crates.io, this is a packaging bug.",
            runtime_src_dir.display(),
            prebuilt.display(),
        );
    }

    let runtime_wasm = build_runtime(&out_dir)?;
    emit_runtime_wasm(&runtime_wasm, &out_dir)
}

fn emit_runtime_wasm(runtime_wasm: &Path, out_dir: &Path) -> Result<()> {
    println!(
        "cargo:rustc-env=RUNTIME_WASM_PATH={}",
        runtime_wasm.display()
    );

    let output = format!(
        r#"const RUNTIME_WASM: &[u8] = include_bytes!({:?});"#,
        runtime_wasm,
    );
    fs::write(out_dir.join("output.rs"), output).context("Failed to write output.rs")?;

    Ok(())
}

fn build_runtime(out_dir: &Path) -> Result<PathBuf> {
    let target = "wasm32-wasip2";
    let upcase = target.to_uppercase().replace('-', "_");

    // Get wasi-sdk - from env, cached, or download
    let wasi_sdk = get_wasi_sdk(out_dir)?;
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

    let clang = executable(&wasi_sdk, "bin/clang");
    let mut cargo = Command::new("cargo");
    if env::var_os(RUNTIME_AUDITABLE_ENV).is_some() {
        cargo.arg("auditable");
    }
    cargo
        .arg("build")
        .arg("--target")
        .arg(target)
        .arg("--package=componentize-qjs-runtime")
        .arg("--no-default-features")
        .env("CARGO_TARGET_DIR", out_dir)
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
        let wasm_opt = get_wasm_opt(out_dir)?;
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

    Ok(runtime_dst)
}

fn get_wasi_sdk(out_dir: &Path) -> Result<PathBuf> {
    // Check environment first
    if let Ok(path) = env::var("WASI_SDK_PATH") {
        let p = PathBuf::from(path);
        if executable(&p, "bin/clang").exists() {
            return Ok(p);
        }
    }

    // Check cached location
    let stable = out_dir.join("wasi-sdk");
    if executable(&stable, "bin/clang").exists() {
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
        .find(|entry| entry.is_dir() && executable(entry, "bin/clang").exists())
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
    let wasm_opt = executable(&stable, "bin/wasm-opt");
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

    Ok(executable(&stable, "bin/wasm-opt"))
}

fn find_binaryen(target_dir: &Path) -> Option<PathBuf> {
    let pattern = target_dir.join("binaryen*");
    glob::glob(pattern.to_str()?)
        .ok()?
        .filter_map(Result::ok)
        .find(|entry| entry.is_dir() && executable(entry, "bin/wasm-opt").exists())
}

fn executable(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.join(relative);
    if !env::consts::EXE_SUFFIX.is_empty() {
        path.set_extension(&env::consts::EXE_SUFFIX[1..]);
    }
    path
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
        .take(MAX_ARCHIVE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("Failed to download archive")?;
    if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
        bail!("Archive exceeds maximum download size of {MAX_ARCHIVE_BYTES} bytes");
    }

    let decoder = GzDecoder::new(bytes.as_slice());

    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(out_dir)
        .context("Failed to extract archive")?;

    Ok(())
}
