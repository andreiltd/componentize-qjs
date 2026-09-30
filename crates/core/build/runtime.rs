//! Portable runtime-artifact preparation shared by xtask and the Cargo fallback.

use std::io::Read;
use std::path::{Path, PathBuf, absolute};
use std::process::Command;
use std::{env, fs};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;

const WASI_SDK_VERSION: &str = "33";
const WASI_SKD_DL_URL: &str = "https://github.com/WebAssembly/wasi-sdk/releases/download";

const BINARYEN_VERSION: &str = "130";
const BINARYEN_DL_URL: &str = "https://github.com/WebAssembly/binaryen/releases/download";
/// Compatibility switch for dependency-auditable source builds.
pub const RUNTIME_AUDITABLE_ENV: &str = "COMPONENTIZE_QJS_RUNTIME_AUDITABLE";
const MAX_ARCHIVE_BYTES: u64 = 1_000_000_000;

/// Description of one embedded runtime artifact.
///
/// `fallback_const` is used for async constants when the async Cargo feature is
/// disabled; in that configuration they alias the corresponding sync runtime.
#[derive(Clone, Copy)]
pub struct RuntimeBuild {
    /// Human-readable variant name.
    pub name: &'static str,
    /// Stable artifact filename, independent of Cargo's OUT_DIR layout.
    pub filename: &'static str,
    /// Constant emitted when the artifact is embedded.
    pub const_name: &'static str,
    /// Whether this artifact is optimized for size rather than speed.
    pub optimize_size: bool,
    /// Whether this artifact requires component-model async support.
    pub async_support: bool,
    /// Sync constant used when async embedding is disabled.
    pub fallback_const: Option<&'static str>,
}

/// All runtime artifacts in generated-constant order.
pub const RUNTIME_BUILDS: [RuntimeBuild; 4] = [
    RuntimeBuild {
        name: "default-sync",
        filename: "runtime-sync.wasm",
        const_name: "DEFAULT_SYNC_RUNTIME_WASM",
        optimize_size: false,
        async_support: false,
        fallback_const: None,
    },
    RuntimeBuild {
        name: "opt-size-sync",
        filename: "runtime-opt-size-sync.wasm",
        const_name: "OPT_SIZE_SYNC_RUNTIME_WASM",
        optimize_size: true,
        async_support: false,
        fallback_const: None,
    },
    RuntimeBuild {
        name: "default",
        filename: "runtime.wasm",
        const_name: "DEFAULT_RUNTIME_WASM",
        optimize_size: false,
        async_support: true,
        fallback_const: Some("DEFAULT_SYNC_RUNTIME_WASM"),
    },
    RuntimeBuild {
        name: "opt-size",
        filename: "runtime-opt-size.wasm",
        const_name: "OPT_SIZE_RUNTIME_WASM",
        optimize_size: true,
        async_support: true,
        fallback_const: Some("OPT_SIZE_SYNC_RUNTIME_WASM"),
    },
];

/// Options common to explicit preparation and automatic source builds.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeOptions {
    /// Optimize the Rust/C runtime and run wasm-opt.
    pub release: bool,
    /// Include the two async artifacts in addition to the sync variants.
    pub async_support: bool,
    /// Build through cargo-auditable to retain dependency metadata.
    pub auditable: bool,
}

/// Compiler settings for one Cargo profile.
struct CargoProfile {
    name: &'static str,
    release: bool,
}

impl CargoProfile {
    /// Select the nested Cargo profile independently of the caller's profile.
    fn new(release: bool) -> Self {
        let name = if release { "release" } else { "debug" };
        Self { name, release }
    }

    /// Link a relocatable runtime with the requested optimization objective.
    fn runtime_rustflags(&self, optimize_size: bool) -> String {
        let flags = "-Clink-arg=-shared -Clink-arg=-Wl,--no-entry -Clink-arg=-Wl,--allow-undefined";

        match (self.release, optimize_size) {
            (true, true) => format!("{flags} -Clto=fat -Copt-level=z"),
            (true, false) => format!("{flags} -Clto=fat -Copt-level=3"),
            (false, _) => flags.to_string(),
        }
    }

    /// Compile QuickJS C sources as position-independent code.
    fn runtime_cflags(&self, optimize_size: bool) -> String {
        let flags = "-fPIC";

        match (self.release, optimize_size) {
            (true, true) => format!("{flags} -Oz"),
            (true, false) => format!("{flags} -O3"),
            (false, _) => flags.to_string(),
        }
    }

    /// Avoid disposable debug/incremental data while respecting caller overrides.
    fn configure_nested_build(&self, cargo: &mut Command) {
        if !self.release {
            set_env_if_unset(cargo, "CARGO_PROFILE_DEV_DEBUG", "0");
        }
        // Runtime target directories are disposable, so incremental state
        // cannot be reused after this build script finishes.
        set_env_if_unset(cargo, "CARGO_INCREMENTAL", "0");
    }
}

/// Shared nested Cargo targets for speed- and size-optimized variants.
///
/// Sync and async builds with the same optimization flags share dependencies.
/// Both directories remain alive until every variant has finished.
struct RuntimeTargetDirs {
    default: PathBuf,
    opt_size: PathBuf,
}

impl RuntimeTargetDirs {
    /// Allocate separate target paths for incompatible optimization flags.
    fn new(out_dir: &Path) -> Self {
        Self {
            default: out_dir.join("runtime-default"),
            opt_size: out_dir.join("runtime-opt-size"),
        }
    }

    /// Share one target directory between sync/async variants with matching flags.
    fn get(&self, optimize_size: bool) -> &Path {
        if optimize_size {
            &self.opt_size
        } else {
            &self.default
        }
    }
}

impl Drop for RuntimeTargetDirs {
    fn drop(&mut self) {
        cleanup_runtime_target_dir(&self.default);
        cleanup_runtime_target_dir(&self.opt_size);
    }
}

/// Prepare all selected runtime variants at their stable artifact paths.
///
/// Validate source/output locations, share nested dependency builds between
/// compatible variants, then copy and optionally optimize each resulting Wasm.
pub fn prepare(
    runtime_dir: &Path,
    out_dir: &Path,
    build_dir: &Path,
    options: RuntimeOptions,
) -> Result<()> {
    if !runtime_dir.join("src").is_dir() {
        bail!(
            "Runtime source not found at {}. If installing from crates.io, \
             missing pre-built runtimes are a packaging bug.",
            runtime_dir.display(),
        );
    }

    fs::create_dir_all(out_dir).context("Failed to create runtime artifact directory")?;
    fs::create_dir_all(build_dir).context("Failed to create runtime build directory")?;

    // Canonicalization introduces Windows verbatim paths that break nested
    // dependencies using include!(concat!(env!("OUT_DIR"), "/bindings.rs")).
    let out_dir = absolute(out_dir).context("Failed to resolve runtime artifact directory")?;
    let build_dir = absolute(build_dir).context("Failed to resolve runtime build directory")?;
    let runtime_dir =
        absolute(runtime_dir).context("Failed to resolve runtime source directory")?;
    let profile = CargoProfile::new(options.release);
    let target_dirs = RuntimeTargetDirs::new(&build_dir);

    for build in RUNTIME_BUILDS {
        if build.async_support && !options.async_support {
            continue;
        }

        build_runtime(
            &runtime_dir,
            &out_dir,
            &build_dir,
            &target_dirs,
            build,
            &profile,
            options.auditable,
        )?;
    }

    Ok(())
}

/// Supply a nested-build default only when the caller did not set it.
fn set_env_if_unset(cargo: &mut Command, key: &str, value: &str) {
    if env::var_os(key).is_none() {
        cargo.env(key, value);
    }
}

/// Build, stage, and optionally optimize one runtime artifact.
fn build_runtime(
    runtime_dir: &Path,
    out_dir: &Path,
    build_dir: &Path,
    target_dirs: &RuntimeTargetDirs,
    build: RuntimeBuild,
    profile: &CargoProfile,
    auditable: bool,
) -> Result<()> {
    let target = "wasm32-wasip2";
    let upcase = target.to_uppercase().replace('-', "_");

    // Get wasi-sdk - from env, cached, or download
    let wasi_sdk = get_wasi_sdk(build_dir)?;
    eprintln!("Using wasi-sdk at: {}", wasi_sdk.display());

    let optimize_size = build.optimize_size;
    let rustflags = profile.runtime_rustflags(optimize_size);
    let cflags = profile.runtime_cflags(optimize_size);

    let clang = executable(&wasi_sdk, "bin/clang");
    let target_dir = target_dirs.get(optimize_size);
    let mut cargo = Command::new("cargo");

    if auditable {
        cargo.arg("auditable");
    }

    cargo
        .current_dir(runtime_dir)
        .arg("build")
        .arg("--target")
        .arg(target)
        .arg("--package=componentize-qjs-runtime")
        .arg("--no-default-features")
        .env("CARGO_TARGET_DIR", target_dir)
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

    if build.async_support {
        cargo.arg("--features").arg("component-model-async");
    }

    eprintln!("Building {} runtime: {cargo:?}", build.name);
    let status = cargo.status().context("Failed to run cargo build")?;
    if !status.success() {
        bail!("Failed to build {} runtime", build.name);
    }

    let runtime_src = target_dir
        .join(target)
        .join(profile.name)
        .join("componentize_qjs_runtime.wasm");

    let runtime_dst = out_dir.join(build.filename);

    fs::copy(&runtime_src, &runtime_dst)
        .with_context(|| format!("Failed to copy {}", runtime_src.display()))?;

    if profile.release {
        let wasm_opt = get_wasm_opt(build_dir)?;
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

    Ok(())
}

/// Remove only the nested target directory owned by this preparation run.
fn cleanup_runtime_target_dir(target_dir: &Path) {
    match fs::remove_dir_all(target_dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            eprintln!(
                "warning: failed to clean nested runtime target dir {}: {err}",
                target_dir.display()
            );
        }
    }
}

/// Resolve the WASI SDK from an override, the output cache, or its pinned release.
fn get_wasi_sdk(out_dir: &Path) -> Result<PathBuf> {
    // Check environment first
    if let Ok(path) = env::var("WASI_SDK_PATH") {
        let p = PathBuf::from(path);
        if executable(&p, "bin/clang").exists() {
            return absolute(p).context("Failed to resolve WASI_SDK_PATH");
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

/// Locate the extracted SDK directory.
fn find_wasi_sdk(target_dir: &Path) -> Option<PathBuf> {
    let pattern = target_dir.join("wasi-sdk*");
    glob::glob(pattern.to_str()?)
        .ok()?
        .filter_map(Result::ok)
        .find(|entry| entry.is_dir() && executable(entry, "bin/clang").exists())
}

/// Resolve wasm-opt from an override, the output cache, or its pinned release.
fn get_wasm_opt(out_dir: &Path) -> Result<PathBuf> {
    // Check WASM_OPT environment variable first
    if let Ok(path) = env::var("WASM_OPT") {
        let p = PathBuf::from(path);
        if p.exists() {
            return absolute(p).context("Failed to resolve WASM_OPT");
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

/// Locate the extracted Binaryen directory.
fn find_binaryen(target_dir: &Path) -> Option<PathBuf> {
    let pattern = target_dir.join("binaryen*");
    glob::glob(pattern.to_str()?)
        .ok()?
        .filter_map(Result::ok)
        .find(|entry| entry.is_dir() && executable(entry, "bin/wasm-opt").exists())
}

/// Apply the host executable suffix to a tool path.
fn executable(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.join(relative);
    if !env::consts::EXE_SUFFIX.is_empty() {
        path.set_extension(&env::consts::EXE_SUFFIX[1..]);
    }
    path
}

/// Select the pinned tool archives for the build host, not the Wasm target.
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

/// Download a size-bounded tool archive and extract it into the artifact cache.
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Each embedding constant and artifact has one definition and a valid fallback.
    #[test]
    fn artifact_manifest_is_consistent() {
        let mut filenames = HashSet::new();
        let mut constants = HashSet::new();
        let mut variants = HashSet::new();

        for build in RUNTIME_BUILDS {
            assert!(filenames.insert(build.filename));
            assert!(variants.insert((build.async_support, build.optimize_size)));

            if let Some(fallback) = build.fallback_const {
                assert!(build.async_support);
                assert!(
                    constants.contains(fallback),
                    "fallback must precede its alias"
                );
            } else {
                assert!(!build.async_support);
            }

            assert!(constants.insert(build.const_name));
        }

        assert_eq!(variants.len(), 4);
    }

    /// Explicit preparation and the Cargo fallback use identical profile flags.
    #[test]
    fn profile_flags_preserve_runtime_variants() {
        let debug = CargoProfile::new(false);
        assert_eq!(debug.name, "debug");
        assert_eq!(
            debug.runtime_rustflags(false),
            debug.runtime_rustflags(true)
        );
        assert_eq!(debug.runtime_cflags(true), "-fPIC");

        let release = CargoProfile::new(true);
        assert_eq!(release.name, "release");
        assert!(
            release
                .runtime_rustflags(false)
                .ends_with("-Clto=fat -Copt-level=3")
        );
        assert!(
            release
                .runtime_rustflags(true)
                .ends_with("-Clto=fat -Copt-level=z")
        );
        assert_eq!(release.runtime_cflags(false), "-fPIC -O3");
        assert_eq!(release.runtime_cflags(true), "-fPIC -Oz");
    }
}
