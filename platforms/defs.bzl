"""Local execution with optional BuildBuddy action-cache reads and uploads."""

load("@prelude//cfg/exec_platform:marker.bzl", "get_exec_platform_marker")

def _archive_configuration_impl(ctx: AnalysisContext) -> list[Provider]:
    """Share target-independent downloads across profiles and runtime variants."""
    archive_platform = PlatformInfo(
        label = str(ctx.label.raw_target()),
        configuration = ConfigurationInfo(constraints = {}, values = {}),
    )

    def transition_impl(platform: PlatformInfo) -> PlatformInfo:
        return archive_platform

    return [DefaultInfo(), TransitionInfo(impl = transition_impl)]

archive_configuration = rule(
    impl = _archive_configuration_impl,
    attrs = {},
    is_configuration_rule = True,
)

def _cache_execution_platform_impl(ctx: AnalysisContext) -> list[Provider]:
    label = ctx.label.raw_target()

    # Recursive target patterns also analyze this optional registration.
    if ctx.attrs.require_namespace and not read_root_config("buck2_re_client", "instance_name", ""):
        fail("Set buck2_re_client.instance_name to a namespace identifying your host OS/native SDK before enabling BuildBuddy.")

    constraints = dict(ctx.attrs.cpu[ConfigurationInfo].constraints)
    constraints.update(ctx.attrs.os[ConfigurationInfo].constraints)
    configuration = ConfigurationInfo(
        constraints = constraints,
        values = {},
    )
    platform = ExecutionPlatformInfo(
        label = label,
        configuration = configuration,
        executor_config = CommandExecutorConfig(
            local_enabled = True,
            remote_enabled = False,
            remote_cache_enabled = True,
            allow_cache_uploads = True,
            remote_execution_use_case = "buck2-default",
            remote_output_paths = "output_paths",
            use_windows_path_separators = ctx.attrs.windows,
        ),
    )

    return [
        DefaultInfo(),
        PlatformInfo(label = str(label), configuration = configuration),
        platform,
        ExecutionPlatformRegistrationInfo(
            platforms = [platform],
            exec_marker_constraint = get_exec_platform_marker(),
        ),
    ]

_cache_execution_platform = rule(
    impl = _cache_execution_platform_impl,
    attrs = {
        "cpu": attrs.dep(providers = [ConfigurationInfo]),
        "os": attrs.dep(providers = [ConfigurationInfo]),
        "require_namespace": attrs.bool(),
        "windows": attrs.bool(),
    },
    is_configuration_rule = True,
)

def cache_execution_platform(name, **kwargs):
    """Require an SDK namespace only when this cache registration is selected."""
    selected = read_root_config("build", "execution_platforms", "").strip()
    selected_cell, selected_target = selected.split("//") if "//" in selected else ("root", selected)
    selected_cell = read_root_config("cell_aliases", selected_cell, selected_cell) if selected_cell else "root"
    target = "{}:{}".format(get_base_path(), name)

    _cache_execution_platform(
        name = name,
        require_namespace = selected_cell == "root" and selected_target == target,
        **kwargs
    )
