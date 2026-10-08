"""Native runtime transitions and relocatable embedding inputs."""

def _wasm_transition_impl(platform, refs, attrs):
    constraints = dict(platform.configuration.constraints)
    constraints[refs.cpu[ConstraintSettingInfo].label] = refs.wasm32[ConstraintValueInfo]
    constraints[refs.os[ConstraintSettingInfo].label] = refs.wasi[ConstraintValueInfo]
    constraints[refs.optimization[ConstraintSettingInfo].label] = (refs.size if attrs.optimize_size else refs.speed)[ConstraintValueInfo]

    return PlatformInfo(
        label = "componentize-qjs-wasip2-" + ("size" if attrs.optimize_size else "speed"),
        configuration = ConfigurationInfo(
            constraints = constraints,
            values = platform.configuration.values,
        ),
    )

_wasm_transition = transition(
    impl = _wasm_transition_impl,
    refs = {
        "cpu": "config//cpu/constraints:cpu",
        "os": "config//os/constraints:os",
        "wasm32": "config//cpu/constraints:wasm32",
        "wasi": "config//os/constraints:wasi",
        "optimization": "//platforms:runtime_optimization",
        "speed": "//platforms:runtime_speed",
        "size": "//platforms:runtime_size",
    },
    attrs = ["optimize_size"],
)

def _wasm_runtime_impl(ctx):
    module = ctx.attrs.module[DefaultInfo].default_outputs[0]
    output = ctx.actions.declare_output(ctx.attrs.filename)

    if ctx.attrs.release:
        ctx.actions.run(
            cmd_args(
                ctx.attrs.wasm_opt[RunInfo],
                "-Oz" if ctx.attrs.optimize_size else "-O3",
                "--all-features",
                "--disable-gc",
                "--disable-reference-types",
                "--strip-debug",
                "--strip-producers",
                module,
                "-o",
                output.as_output(),
            ),
            category = "optimize_runtime",
        )
    else:
        ctx.actions.copy_file(output, module)

    return [DefaultInfo(default_output = output)]

wasm_runtime = rule(
    impl = _wasm_runtime_impl,
    attrs = {
        "filename": attrs.string(),
        "module": attrs.transition_dep(cfg = _wasm_transition),
        "optimize_size": attrs.bool(default = False),
        "release": attrs.bool(default = False),
        "wasm_opt": attrs.exec_dep(default = "toolchains//:wasm-opt", providers = [RunInfo]),
    },
)

def _runtime_embeddings_impl(ctx):
    files = {}
    source = []

    for constant, runtime in ctx.attrs.runtimes.items():
        artifact = runtime[DefaultInfo].default_outputs[0]
        files[artifact.basename] = artifact
        source.append('const {}: &[u8] = include_bytes!("{}");'.format(constant, artifact.basename))

    if not ctx.attrs.async_support:
        source.extend([
            "const DEFAULT_RUNTIME_WASM: &[u8] = DEFAULT_SYNC_RUNTIME_WASM;",
            "const OPT_SIZE_RUNTIME_WASM: &[u8] = OPT_SIZE_SYNC_RUNTIME_WASM;",
        ])

    files["output.rs"] = ctx.actions.write("output.rs", "\n".join(source) + "\n")
    output = ctx.actions.symlinked_dir("embeddings", files)

    return [DefaultInfo(default_output = output)]

runtime_embeddings = rule(
    impl = _runtime_embeddings_impl,
    attrs = {
        "async_support": attrs.bool(default = True),
        "runtimes": attrs.dict(attrs.string(), attrs.dep()),
    },
)
