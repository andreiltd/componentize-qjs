load("//buck:project.bzl", "cargo_package", "cli_targets", "core_library", "napi_targets", "runtime_targets", "xtask_targets")
load("//crates/core:Cargo.toml", core_manifest = "value")
load("//crates/runtime:Cargo.toml", runtime_manifest = "value")
load("//napi:Cargo.toml", napi_manifest = "value")
load("//tests:defs.bzl", "node_tests")
load("//xtask:Cargo.toml", xtask_manifest = "value")
load(":Cargo.toml", cli_manifest = "value")

_WORKSPACE_PACKAGE = cli_manifest["workspace"]["package"]

core_library(
    name = "componentize-qjs-lib",
    crate_root = "crates/core/src/lib.rs",
    srcs = glob(["crates/core/src/**", "crates/core/wit/**"]) + [
        "Cargo.toml",
        "crates/core/Cargo.toml",
    ],
    deps = [
        "//third-party:anyhow",
        "//third-party:bytes",
        "//third-party:indexmap",
        "//third-party:oxc_resolver",
        "//third-party:tempfile",
        "//third-party:tokio",
        "//third-party:wasi-preview1-component-adapter-provider",
        "//third-party:wasm-compose",
        "//third-party:wasmtime",
        "//third-party:wasmtime-wasi",
        "//third-party:wasmtime-wizer",
        "//third-party:wit-component",
        "//third-party:wit-dylib",
        "//third-party:wit-parser",
    ],
    visibility = ["PUBLIC"],
    **cargo_package(core_manifest, _WORKSPACE_PACKAGE, "crates/core")
)

cli_targets(
    name = "componentize-qjs-cli-lib",
    crate_root = "src/lib.rs",
    srcs = glob(["src/**/*.rs"], exclude = ["src/main.rs"]) + ["Cargo.toml"],
    deps = [
        ":componentize-qjs-lib",
        "//third-party:anyhow",
        "//third-party:clap",
        "//third-party:oxc_allocator",
        "//third-party:oxc_codegen",
        "//third-party:oxc_minifier",
        "//third-party:oxc_parser",
        "//third-party:oxc_span",
        "//third-party:tokio",
    ],
    visibility = ["PUBLIC"],
    **cargo_package(cli_manifest, _WORKSPACE_PACKAGE)
)

runtime_targets(
    name = "componentize-qjs-runtime",
    crate_root = "crates/runtime/src/lib.rs",
    srcs = glob(["crates/runtime/src/**/*.rs"]) + [
        "Cargo.toml",
        "crates/runtime/Cargo.toml",
        "crates/core/wit/init.wit",
    ],
    deps = [
        "//third-party:heck",
        "//third-party:indexmap",
        "//third-party:num_enum",
        "//third-party:rquickjs",
        "//third-party:smallvec",
        "//third-party:wit-bindgen",
        "//third-party:wit-dylib-ffi",
    ],
    **cargo_package(runtime_manifest, _WORKSPACE_PACKAGE, "crates/runtime")
)

napi_targets(
    name = "napi-lib",
    crate_root = "napi/src/lib.rs",
    srcs = glob(["napi/src/**/*.rs"]) + [
        "Cargo.toml",
        "napi/Cargo.toml",
    ],
    deps = [
        ":componentize-qjs-lib",
        ":componentize-qjs-cli-lib",
        "//third-party:anyhow",
        "//third-party:clap",
        "//third-party:napi",
        "//third-party:napi-derive",
        "//third-party:tokio",
    ],
    **cargo_package(napi_manifest, _WORKSPACE_PACKAGE, "napi")
)

xtask_targets(
    name = "xtask",
    crate_root = "xtask/src/main.rs",
    srcs = glob([
        "xtask/**/*.rs",
        "crates/**/*.rs",
        "crates/**/Cargo.toml",
        "crates/core/wit/**",
        "napi/**/*.rs",
        "src/**/*.rs",
    ]) + [
        "Cargo.toml",
        "Cargo.lock",
        "napi/Cargo.toml",
        "xtask/Cargo.toml",
    ],
    deps = [
        "//third-party:anyhow",
        "//third-party:clap",
        "//third-party:flate2",
        "//third-party:glob",
        "//third-party:tar",
        "//third-party:ureq",
    ],
    visibility = ["PUBLIC"],
    **cargo_package(xtask_manifest, _WORKSPACE_PACKAGE, "xtask")
)

node_tests(name = "node-test")
