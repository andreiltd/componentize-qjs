use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;

const WASI_SDK_VERSION: &str = "33";
const WASI_SKD_DL_URL: &str = "https://github.com/WebAssembly/wasi-sdk/releases/download";

const BINARYEN_VERSION: &str = "130";
const BINARYEN_DL_URL: &str = "https://github.com/WebAssembly/binaryen/releases/download";
const RUNTIME_AUDITABLE_ENV: &str = "COMPONENTIZE_QJS_RUNTIME_AUDITABLE";
const MAX_ARCHIVE_BYTES: u64 = 1_000_000_000;

#[derive(Clone, Copy)]
struct RuntimeBuild {
    optimize_size: bool,
    async_support: bool,
}

impl RuntimeBuild {
    const DEFAULT: Self = Self {
        optimize_size: false,
        async_support: true,
    };
    const OPT_SIZE: Self = Self {
        optimize_size: true,
        async_support: true,
    };
    const DEFAULT_SYNC: Self = Self {
        optimize_size: false,
        async_support: false,
    };
    const OPT_SIZE_SYNC: Self = Self {
        optimize_size: true,
        async_support: false,
    };

    fn name(self) -> &'static str {
        match (self.optimize_size, self.async_support) {
            (false, true) => "default",
            (true, true) => "opt-size",
            (false, false) => "default-sync",
            (true, false) => "opt-size-sync",
        }
    }

    fn filename(self) -> &'static str {
        match (self.optimize_size, self.async_support) {
            (false, true) => "runtime.wasm",
            (true, true) => "runtime-opt-size.wasm",
            (false, false) => "runtime-sync.wasm",
            (true, false) => "runtime-opt-size-sync.wasm",
        }
    }

    fn optimize_size(self) -> bool {
        self.optimize_size
    }

    fn async_support(self) -> bool {
        self.async_support
    }
}

struct CargoProfile {
    name: String,
    release: bool,
}

impl CargoProfile {
    fn current() -> Self {
        let name = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
        let release = name == "release";
        Self { name, release }
    }

    fn runtime_rustflags(&self, optimize_size: bool) -> String {
        let flags = "-Clink-arg=-shared -Clink-arg=-Wl,--no-entry -Clink-arg=-Wl,--allow-undefined";
        match (self.release, optimize_size) {
            (true, true) => format!("{flags} -Clto=fat -Copt-level=z"),
            (true, false) => format!("{flags} -Clto=fat -Copt-level=3"),
            (false, _) => flags.to_string(),
        }
    }

    fn runtime_cflags(&self, optimize_size: bool) -> String {
        let flags = "-fPIC";
        match (self.release, optimize_size) {
            (true, true) => format!("{flags} -Oz"),
            (true, false) => format!("{flags} -O3"),
            (false, _) => flags.to_string(),
        }
    }

    fn configure_nested_build(&self, cargo: &mut Command) {
        if !self.release {
            set_env_if_unset(cargo, "CARGO_PROFILE_DEV_DEBUG", "0");
        }
        set_env_if_unset(cargo, "CARGO_INCREMENTAL", "0");
    }
}

/// Resolved Wasm paths for each embedded runtime variant.
///
/// The non-async variants are always present. The async variants are `None`
/// when the `component-model-async` feature is disabled, in which case the
/// generated `DEFAULT_RUNTIME_WASM` / `OPT_SIZE_RUNTIME_WASM` constants alias
/// the non-async variants (preserving the historical non-async-by-default
/// behavior).
struct RuntimePaths {
    default_sync: PathBuf,
    opt_size_sync: PathBuf,
    default_async: Option<PathBuf>,
    opt_size_async: Option<PathBuf>,
}

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
    println!("cargo:rerun-if-changed=prebuilt/runtime-opt-size.wasm");
    println!("cargo:rerun-if-changed=prebuilt/runtime-sync.wasm");
    println!("cargo:rerun-if-changed=prebuilt/runtime-opt-size-sync.wasm");
    println!("cargo:rerun-if-env-changed={RUNTIME_AUDITABLE_ENV}");

    let out_dir = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR not set")?);
    let async_on = component_model_async_enabled();

    // Check for pre-built runtimes (used when installing from crates.io)
    let prebuilt_dir = manifest_dir.join("prebuilt");
    let prebuilt_sync = prebuilt_dir.join("runtime-sync.wasm");

    if prebuilt_sync.exists() {
        return emit_from_prebuilt(&prebuilt_dir, async_on, &out_dir);
    }

    // Check that runtime source is available (won't be when installed from crates.io
    // without a pre-built runtime)
    let runtime_src_dir = runtime_dir.join("src");
    if !runtime_src_dir.exists() {
        bail!(
            "Runtime source not found at {} and no pre-built runtime at {}. \
             If installing from crates.io, this is a packaging bug.",
            runtime_src_dir.display(),
            prebuilt_sync.display(),
        );
    }

    let profile = CargoProfile::current();
    // Non-async runtimes are always embedded; async runtimes only when the feature is on.
    let default_sync = build_runtime(&out_dir, RuntimeBuild::DEFAULT_SYNC, &profile)?;
    let opt_size_sync = build_runtime(&out_dir, RuntimeBuild::OPT_SIZE_SYNC, &profile)?;
    let (default_async, opt_size_async) = if async_on {
        (
            Some(build_runtime(&out_dir, RuntimeBuild::DEFAULT, &profile)?),
            Some(build_runtime(&out_dir, RuntimeBuild::OPT_SIZE, &profile)?),
        )
    } else {
        (None, None)
    };

    emit_runtime_wasms(
        &RuntimePaths {
            default_sync,
            opt_size_sync,
            default_async,
            opt_size_async,
        },
        &out_dir,
    )
}

/// Emit runtime constants from the pre-built runtimes packaged with the crate.
fn emit_from_prebuilt(prebuilt_dir: &Path, async_on: bool, out_dir: &Path) -> Result<()> {
    let default_sync = prebuilt_dir.join("runtime-sync.wasm");
    let opt_size_sync = prebuilt_dir.join("runtime-opt-size-sync.wasm");

    if !opt_size_sync.exists() {
        bail!(
            "Pre-built non-async runtime exists at {} but opt-size non-async runtime is missing \
             at {}. If installing from crates.io, this is a packaging bug.",
            default_sync.display(),
            opt_size_sync.display(),
        );
    }

    let (default_async, opt_size_async) = if async_on {
        let default_async = prebuilt_dir.join("runtime.wasm");
        let opt_size_async = prebuilt_dir.join("runtime-opt-size.wasm");
        if !default_async.exists() || !opt_size_async.exists() {
            bail!(
                "Pre-built async runtimes are missing at {} / {} while the \
                 component-model-async feature is enabled. If installing from crates.io, \
                 this is a packaging bug.",
                default_async.display(),
                opt_size_async.display(),
            );
        }
        (Some(default_async), Some(opt_size_async))
    } else {
        (None, None)
    };

    eprintln!("Using prebuilt runtimes from: {}", prebuilt_dir.display());

    emit_runtime_wasms(
        &RuntimePaths {
            default_sync,
            opt_size_sync,
            default_async,
            opt_size_async,
        },
        out_dir,
    )
}

fn emit_runtime_wasms(paths: &RuntimePaths, out_dir: &Path) -> Result<()> {
    let mut output = String::new();
    output.push_str(&const_line(
        "DEFAULT_SYNC_RUNTIME_WASM",
        &paths.default_sync,
    ));
    output.push_str(&const_line(
        "OPT_SIZE_SYNC_RUNTIME_WASM",
        &paths.opt_size_sync,
    ));

    match &paths.default_async {
        Some(path) => output.push_str(&const_line("DEFAULT_RUNTIME_WASM", path)),
        None => output.push_str("const DEFAULT_RUNTIME_WASM: &[u8] = DEFAULT_SYNC_RUNTIME_WASM;\n"),
    }
    match &paths.opt_size_async {
        Some(path) => output.push_str(&const_line("OPT_SIZE_RUNTIME_WASM", path)),
        None => {
            output.push_str("const OPT_SIZE_RUNTIME_WASM: &[u8] = OPT_SIZE_SYNC_RUNTIME_WASM;\n")
        }
    }

    fs::write(out_dir.join("output.rs"), output).context("Failed to write output.rs")?;

    Ok(())
}

fn const_line(name: &str, path: &Path) -> String {
    format!("const {name}: &[u8] = include_bytes!({path:?});\n")
}

fn set_env_if_unset(cargo: &mut Command, key: &str, value: &str) {
    if env::var_os(key).is_none() {
        cargo.env(key, value);
    }
}

fn build_runtime(out_dir: &Path, build: RuntimeBuild, profile: &CargoProfile) -> Result<PathBuf> {
    let target = "wasm32-wasip2";
    let upcase = target.to_uppercase().replace('-', "_");

    // Get wasi-sdk - from env, cached, or download
    let wasi_sdk = get_wasi_sdk(out_dir)?;
    eprintln!("Using wasi-sdk at: {}", wasi_sdk.display());

    let optimize_size = build.optimize_size();
    let rustflags = profile.runtime_rustflags(optimize_size);
    let cflags = profile.runtime_cflags(optimize_size);

    let clang = executable(&wasi_sdk, "bin/clang");
    let target_dir = out_dir.join(format!("runtime-{}", build.name()));
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
        .env("CARGO_TARGET_DIR", &target_dir)
        .env(format!("CARGO_TARGET_{upcase}_RUSTFLAGS"), rustflags)
        .env(format!("CARGO_TARGET_{upcase}_LINKER"), &clang)
        .env(format!("CFLAGS_{}", target.replace('-', "_")), cflags)
        .env(format!("CC_{}", target.replace('-', "_")), &clang)
        .env("WASI_SDK_PATH", &wasi_sdk)
        .env("WASI_SDK", &wasi_sdk)
        .env_remove("CARGO_ENCODED_RUSTFLAGS");

    profile.configure_nested_build(&mut cargo);

    if profile.release {
        cargo.arg("--release");
    }

    if build.async_support() {
        cargo.arg("--features").arg("component-model-async");
    }

    eprintln!("Building {} runtime: {cargo:?}", build.name());
    let status = cargo.status().context("Failed to run cargo build")?;
    if !status.success() {
        bail!("Failed to build {} runtime", build.name());
    }

    let runtime_src = target_dir
        .join(target)
        .join(&profile.name)
        .join("componentize_qjs_runtime.wasm");

    let runtime_dst = out_dir.join(build.filename());

    fs::copy(&runtime_src, &runtime_dst)
        .with_context(|| format!("Failed to copy {}", runtime_src.display()))?;

    if profile.release {
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

    cleanup_runtime_target_dir(&target_dir);

    Ok(runtime_dst)
}

fn cleanup_runtime_target_dir(target_dir: &Path) {
    if let Err(err) = fs::remove_dir_all(target_dir) {
        eprintln!(
            "warning: failed to clean nested runtime target dir {}: {err}",
            target_dir.display()
        );
    }
}

fn component_model_async_enabled() -> bool {
    env::var_os("CARGO_FEATURE_COMPONENT_MODEL_ASYNC").is_some()
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
