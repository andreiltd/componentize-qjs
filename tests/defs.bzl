"""Buck integration-test declarations with Cargo-compatible source inputs."""

load("@prelude//:prelude.bzl", "native")

def node_tests(name):
    """Run the addon smoke test through Buck's declared Node interpreter."""
    native.export_file(name = "node-test-source", src = "tests/node.cjs")
    native.command_alias(
        name = "node-smoke",
        exe = "toolchains//:node",
        args = ["$(location :node-test-source)", "$(location :componentize-qjs-node)"],
        visibility = ["PUBLIC"],
    )
    native.sh_test(
        name = name,
        test = ":node-smoke",
    )

def integration_tests(srcs, env, deps, features, rustc_flags):
    """Expose the existing Cargo integration suites with declared runtime inputs."""
    for suite in ["cli", "wit_types", "async_types", "wasi", "fuzz"]:
        native.rust_test(
            name = suite.replace("_", "-") + "-test",
            crate = suite,
            crate_root = "tests/" + suite + ".rs",
            srcs = ["tests/" + suite + ".rs", "tests/common/mod.rs", "Cargo.toml"] + srcs,
            env = env,
            run_env = {
                "COMPONENTIZE_QJS_TEST_CLI": "$(location :componentize-qjs)",
                "COMPONENTIZE_QJS_TEST_ROOT": "$(location :test-data)",
            },
            deps = deps,
            features = features,
            rustc_flags = rustc_flags,
            link_style = "static",
        )
