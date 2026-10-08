"""First-party Cargo metadata, profiles, tests, and native runtime integration."""

load("@prelude//:prelude.bzl", "native")
load("//tests:defs.bzl", "integration_tests")
load(":config.bzl", "cargo_features", "enabled_feature", "profile_select")
load(":runtime.bzl", "runtime_embeddings", "wasm_runtime")

def _package_value(package, workspace, key, default = ""):
    """Resolve a Cargo package field inherited from workspace.package."""
    value = package.get(key, default)

    if type(value) == "dict" and value.get("workspace"):
        return workspace[key]

    return value

def cargo_package(manifest, workspace, manifest_dir = "."):
    """Read Cargo's package identity and compile-time environment without generation."""
    package = manifest["package"]
    version = _package_value(package, workspace, "version")
    release = version.split("+", 1)[0].split("-", 1)
    major, minor, patch = release[0].split(".")
    crate = manifest.get("lib", {}).get("name", package["name"].replace("-", "_"))

    return {
        "crate": crate,
        "edition": _package_value(package, workspace, "edition", "2015"),
        "env": {
            "CARGO_BIN_NAME": crate,
            "CARGO_CRATE_NAME": crate,
            "CARGO_MANIFEST_DIR": manifest_dir,
            "CARGO_PKG_AUTHORS": ":".join(_package_value(package, workspace, "authors", [])),
            "CARGO_PKG_DESCRIPTION": _package_value(package, workspace, "description"),
            "CARGO_PKG_HOMEPAGE": _package_value(package, workspace, "homepage"),
            "CARGO_PKG_NAME": package["name"],
            "CARGO_PKG_README": _package_value(package, workspace, "readme"),
            "CARGO_PKG_REPOSITORY": _package_value(package, workspace, "repository"),
            "CARGO_PKG_RUST_VERSION": _package_value(package, workspace, "rust-version"),
            "CARGO_PKG_VERSION": version,
            "CARGO_PKG_VERSION_MAJOR": major,
            "CARGO_PKG_VERSION_MINOR": minor,
            "CARGO_PKG_VERSION_PATCH": patch,
            "CARGO_PKG_VERSION_PRE": release[1] if len(release) == 2 else "",
        },
    }

def _unit_test(name, attrs):
    native.rust_test(name = name, link_style = "static", **attrs)

def _library_attrs(kwargs):
    """Apply the workspace's feature switches without inferring a target's role."""
    attrs = dict(kwargs)
    attrs["env"] = dict(attrs["env"])
    attrs["features"] = cargo_features()
    return attrs

def core_library(name, **kwargs):
    """Build the core library and unit tests with declared runtime embeddings."""
    attrs = _library_attrs(kwargs)
    attrs["env"]["OUT_DIR"] = "$(location :runtime-embeddings)"
    native.rust_library(name = name, **attrs)
    _unit_test("core-unit", attrs)

def runtime_targets(name, **kwargs):
    """Declare sync/async speed/size Wasm modules, embeddings, and runtime tests."""
    attrs = _library_attrs(kwargs)
    runtime_attrs = dict(attrs)
    runtime_attrs["rustc_flags"] = [
        "-Cdebuginfo=0",
        "-Clinker-flavor=gcc",
        "-Clink-arg=-shared",
        "-Clink-arg=-Wl,--no-entry",
        "-Clink-arg=-Wl,--allow-undefined",
    ] + profile_select(dev = [], test = [], release = ["-Clto=fat", "-Cembed-bitcode=yes"])

    for module, features in [("sync", []), ("async", ["component-model-async"])]:
        runtime_attrs["features"] = features
        native.rust_library(name = "runtime-" + module + "-module", **runtime_attrs)

    native.alias(name = name, actual = ":runtime-sync-module", visibility = ["PUBLIC"])

    for target, module, filename, size in [
        ("runtime", "async", "runtime.wasm", False),
        ("runtime-sync", "sync", "runtime-sync.wasm", False),
        ("runtime-opt-size", "async", "runtime-opt-size.wasm", True),
        ("runtime-opt-size-sync", "sync", "runtime-opt-size-sync.wasm", True),
    ]:
        wasm_runtime(
            name = target,
            module = ":runtime-" + module + "-module[cdylib]",
            filename = filename,
            optimize_size = size,
            release = profile_select(dev = False, test = False, release = True),
            visibility = ["PUBLIC"],
        )

    native.filegroup(
        name = "runtimes",
        srcs = [":runtime", ":runtime-sync", ":runtime-opt-size", ":runtime-opt-size-sync"],
        visibility = ["PUBLIC"],
    )

    async_support = enabled_feature("async_support", "true")

    runtimes = {
        "DEFAULT_SYNC_RUNTIME_WASM": ":runtime-sync",
        "OPT_SIZE_SYNC_RUNTIME_WASM": ":runtime-opt-size-sync",
    }

    if async_support:
        runtimes.update({
            "DEFAULT_RUNTIME_WASM": ":runtime",
            "OPT_SIZE_RUNTIME_WASM": ":runtime-opt-size",
        })

    runtime_embeddings(name = "runtime-embeddings", runtimes = runtimes, async_support = async_support)

    unit_attrs = dict(attrs)
    unit_attrs["features"] = ["component-model-async"] if async_support else []
    unit_attrs["deps"] = attrs["deps"] + ["//third-party:quickcheck"]
    _unit_test("runtime-unit", unit_attrs)

def cli_targets(name, **kwargs):
    """Declare the CLI library, executable, unit tests, and integration suites."""
    attrs = _library_attrs(kwargs)
    attrs["rustc_flags"] = profile_select(dev = [], test = ["-Copt-level=1"], release = [])
    native.rust_library(name = name, **attrs)
    _unit_test("cli-unit", attrs)
    binary_attrs = dict(attrs)
    binary_attrs["crate"] = "componentize_qjs"
    binary_attrs["crate_root"] = "src/main.rs"
    binary_attrs["srcs"] = attrs["srcs"] + ["src/main.rs"]
    binary_attrs["deps"] = attrs["deps"] + [":" + name]
    binary_attrs["visibility"] = ["PUBLIC"]
    binary_attrs["rustc_flags"] = attrs["rustc_flags"] + profile_select(dev = [], test = [], release = ["-Clto=fat", "-Cembed-bitcode=yes"])

    native.rust_binary(name = "componentize-qjs", link_style = "static", **binary_attrs)
    fixtures = native.glob(["tests/wit/**", "tests/js/**", "examples/wasi-stdio/**"])
    native.filegroup(name = "test-data", srcs = fixtures)
    test_env = dict(attrs["env"])
    test_env["CARGO_BIN_EXE_componentize-qjs"] = "$(location :componentize-qjs)"
    test_deps = attrs["deps"] + [":" + name] + [
        "//third-party:assert_cmd",
        "//third-party:predicates",
        "//third-party:quickcheck",
        "//third-party:tempfile",
        "//third-party:wasmtime",
        "//third-party:wasmtime-wasi",
        "//third-party:wit-parser",
    ]
    integration_tests(
        srcs = fixtures,
        env = test_env,
        deps = test_deps,
        features = attrs["features"],
        rustc_flags = attrs["rustc_flags"],
    )

def napi_targets(name, **kwargs):
    """Build the NAPI library and its loadable Node addon."""
    attrs = _library_attrs(kwargs)
    attrs["rustc_flags"] = profile_select(dev = [], test = [], release = ["-Clto=fat", "-Cembed-bitcode=yes"])
    attrs["linker_flags"] = select({
        "config//os:macos": ["-undefined", "dynamic_lookup"],
        "config//os:linux": ["-Wl,-z,nodelete"],
        "DEFAULT": [],
    })
    native.rust_library(name = name, **attrs)
    native.export_file(
        name = "componentize-qjs-node",
        src = ":" + name + "[cdylib]",
        out = "componentize-qjs.node",
        visibility = ["PUBLIC"],
    )

def xtask_targets(name, **kwargs):
    """Expose the existing xtask without changing its explicit Cargo workflow."""
    attrs = dict(kwargs)
    native.rust_binary(name = name, link_style = "static", **attrs)
    _unit_test("xtask-unit", attrs)
