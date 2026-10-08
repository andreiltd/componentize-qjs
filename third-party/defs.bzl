"""Reindeer build scripts with the same profile defaults as first-party code."""

load("@prelude//:prelude.bzl", "native")
load("@prelude//http_archive:exec_deps.bzl", "HttpArchiveExecDeps")
load("@prelude//http_archive:http_archive.bzl", "http_archive_impl")
load("@prelude//rust:cargo_buildscript.bzl", _buildscript_run = "buildscript_run")
load("//buck:config.bzl", "profile_select")

def _crate_download_impl(ctx: AnalysisContext) -> list[Provider]:
    """Reuse archive handling without exposing downloads as project tests."""
    return [http_archive_impl(ctx)[0]]

_crate_download = rule(
    impl = _crate_download_impl,
    attrs = {
        "urls": attrs.list(attrs.string()),
        "vpnless_urls": attrs.list(attrs.string(), default = []),
        "sha1": attrs.option(attrs.string(), default = None),
        "sha256": attrs.string(),
        "size_bytes": attrs.option(attrs.int(), default = None),
        "strip_prefix": attrs.option(attrs.string(), default = None),
        "type": attrs.option(attrs.string(), default = None),
        "out": attrs.option(attrs.string(), default = None),
        "excludes": attrs.list(attrs.string(), default = []),
        "sub_targets": attrs.list(attrs.string(), default = []),
        "has_content_based_path": attrs.bool(default = True),
        "exec_deps": attrs.exec_dep(
            default = "prelude//http_archive/tools:exec_deps",
            providers = [HttpArchiveExecDeps],
        ),
    },
    supports_incoming_transition = True,
)

def crate_download(**kwargs):
    """Share content-addressed archives across their consumers' target variants."""
    _crate_download(incoming_transition = "root//platforms:archives", **kwargs)

def buildscript_run(name, buildscript_rule, env = {}, **kwargs):
    """Supply profile defaults and the selected macOS SDK without helper scripts."""
    defaults = {
        "DEBUG": profile_select(dev = "true", test = "true", release = "false"),
        "NUM_JOBS": "1",
        "OPT_LEVEL": profile_select(
            dev = "0",
            test = "3",
            release = select({
                "root//platforms:runtime_size": "z",
                "DEFAULT": "3",
            }),
        ),
        "PROFILE": profile_select(dev = "debug", test = "debug", release = "release"),
        "LIBCLANG_PATH": "$(location toolchains//:llvm)/" + ("bin" if host_info().os.is_windows else "lib"),
    }

    defaults.update(env)

    if host_info().os.is_macos:
        sdk_wrapper = name + "-sdk"
        native.command_alias(
            name = sdk_wrapper,
            # Build scripts expose one binary; location preserves its target configuration.
            args = ["/usr/bin/xcrun", "--sdk", "macosx", "$(location {})".format(buildscript_rule)],
            run_using_single_arg = True,
        )
        buildscript_rule = ":" + sdk_wrapper

    _buildscript_run(name = name, buildscript_rule = buildscript_rule, env = defaults, **kwargs)
