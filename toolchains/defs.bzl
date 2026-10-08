"""Artifact-backed native Rust/LLVM and WASI C/C++ toolchains.

Executables retain every declared output of their distribution as an input.
"""

load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "BinaryUtilitiesInfo",
    "CCompilerInfo",
    "CxxCompilerInfo",
    "CxxInternalTools",
    "LinkerInfo",
    "LinkerType",
    "ShlibInterfacesMode",
    "cxx_toolchain_infos",
)
load("@prelude//cxx:headers.bzl", "HeaderMode")
load("@prelude//linking:link_info.bzl", "LinkStyle")
load("@prelude//rust:rust_toolchain.bzl", "PanicRuntime", "RustToolchainInfo")
load("@prelude//toolchains:cxx.bzl", "CxxToolsInfo")

def _downloaded_rust_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    compiler_info = ctx.attrs.compiler[DefaultInfo]
    compiler = compiler_info.default_outputs[0]
    compiler_inputs = compiler_info.default_outputs + compiler_info.other_outputs

    native_std = ctx.attrs.native_std[DefaultInfo].default_outputs[0]
    wasi_std = ctx.attrs.wasi_std[DefaultInfo].default_outputs[0]

    sysroot = ctx.actions.symlinked_dir("sysroot", {
        "lib/rustlib/" + ctx.attrs.host_triple + "/lib": native_std.project("lib/rustlib/" + ctx.attrs.host_triple + "/lib"),
        "lib/rustlib/" + ctx.attrs.host_triple + "/bin": compiler.project("lib/rustlib/" + ctx.attrs.host_triple + "/bin"),
        "lib/rustlib/wasm32-wasip2/lib": wasi_std.project("lib/rustlib/wasm32-wasip2/lib"),
    })
    suffix = ".exe" if host_info().os.is_windows else ""

    return [
        DefaultInfo(),
        RustToolchainInfo(
            compiler = RunInfo(args = cmd_args(compiler.project("bin/rustc" + suffix), hidden = compiler_inputs)),
            clippy_driver = ctx.attrs.clippy[RunInfo],
            rustdoc = RunInfo(args = cmd_args(compiler.project("bin/rustdoc" + suffix), hidden = compiler_inputs)),
            sysroot_path = sysroot,
            rustc_target_triple = ctx.attrs.target_triple,
            default_edition = "2024",
            panic_runtime = PanicRuntime("unwind"),
            rustc_flags = ctx.attrs.rustc_flags,
        ),
    ]

downloaded_rust_toolchain = rule(
    impl = _downloaded_rust_toolchain_impl,
    attrs = {
        "compiler": attrs.exec_dep(providers = [DefaultInfo]),
        "clippy": attrs.exec_dep(providers = [RunInfo]),
        "native_std": attrs.exec_dep(providers = [DefaultInfo]),
        "wasi_std": attrs.exec_dep(providers = [DefaultInfo]),
        "host_triple": attrs.string(),
        "target_triple": attrs.string(),
        "rustc_flags": attrs.list(attrs.arg()),
    },
    is_toolchain_rule = True,
)

def _llvm_tools_impl(ctx: AnalysisContext) -> list[Provider]:
    archive_info = ctx.attrs.archive[DefaultInfo]
    archive = archive_info.default_outputs[0]
    archive_inputs = archive_info.default_outputs + archive_info.other_outputs

    windows = host_info().os.is_windows
    macos = host_info().os.is_macos
    suffix = ".exe" if windows else ""

    def tool(name):
        command = cmd_args(archive.project("bin/" + name + suffix), hidden = archive_inputs)

        if macos and name in ["clang", "clang++"]:
            return cmd_args("/usr/bin/xcrun", "--sdk", "macosx", command)

        return command

    return [
        DefaultInfo(),
        CxxToolsInfo(
            compiler = tool("clang-cl" if windows else "clang"),
            compiler_type = "clang_cl" if windows else "clang",
            cxx_compiler = tool("clang-cl" if windows else "clang++"),
            asm_compiler = tool("clang"),
            asm_compiler_type = "clang",
            rc_compiler = tool("llvm-rc") if windows else None,
            cvtres_compiler = tool("llvm-cvtres") if windows else None,
            archiver = tool("llvm-lib" if windows else "llvm-ar"),
            archiver_type = "windows" if windows else "gnu",
            linker = tool("lld-link" if windows else "clang++"),
            linker_type = LinkerType("windows" if windows else "darwin" if macos else "gnu"),
        ),
    ]

llvm_tools = rule(
    impl = _llvm_tools_impl,
    attrs = {"archive": attrs.exec_dep(providers = [DefaultInfo])},
)

def _archive_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    archive_info = ctx.attrs.archive[DefaultInfo]
    archive = archive_info.default_outputs[0]
    archive_inputs = archive_info.default_outputs + archive_info.other_outputs
    binary = archive.project(ctx.attrs.path)

    return [
        DefaultInfo(default_output = binary, other_outputs = archive_inputs),
        RunInfo(args = cmd_args(binary, hidden = archive_inputs)),
    ]

archive_binary = rule(
    impl = _archive_binary_impl,
    attrs = {
        "archive": attrs.exec_dep(providers = [DefaultInfo]),
        "path": attrs.string(),
    },
)

def _wasi_cxx_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    sdk_info = ctx.attrs.sdk[DefaultInfo]
    sdk = sdk_info.default_outputs[0]
    sdk_inputs = sdk_info.default_outputs + sdk_info.other_outputs
    suffix = ".exe" if host_info().os.is_windows else ""

    def tool(name):
        return RunInfo(args = cmd_args(sdk.project("bin/" + name + suffix), hidden = sdk_inputs))

    compiler_flags = cmd_args("--target=wasm32-wasip2", cmd_args(sdk.project("share/wasi-sysroot"), format = "--sysroot={}"), "-fPIC")

    return [DefaultInfo()] + cxx_toolchain_infos(
        platform_name = "wasm32-wasip2",
        internal_tools = ctx.attrs._internal_tools[CxxInternalTools],
        c_compiler_info = CCompilerInfo(
            compiler = tool("clang"),
            compiler_type = "clang",
            compiler_flags = compiler_flags,
            preprocessor_flags = [],
        ),
        cxx_compiler_info = CxxCompilerInfo(
            compiler = tool("clang++"),
            compiler_type = "clang",
            compiler_flags = compiler_flags,
            preprocessor_flags = [],
        ),
        linker_info = LinkerInfo(
            archiver = tool("llvm-ar"),
            archiver_type = "gnu",
            archiver_supports_argfiles = True,
            archive_objects_locally = False,
            binary_extension = ".wasm",
            generate_linker_maps = False,
            link_binaries_locally = False,
            link_libraries_locally = False,
            link_style = LinkStyle("static_pic"),
            link_weight = 1,
            linker = tool("clang"),
            linker_flags = cmd_args("--target=wasm32-wasip2", cmd_args(sdk.project("share/wasi-sysroot"), format = "--sysroot={}")),
            object_file_extension = "o",
            shlib_interfaces = ShlibInterfacesMode("disabled"),
            shared_dep_runtime_ld_flags = [],
            shared_library_name_default_prefix = "",
            shared_library_name_format = "{}.wasm",
            shared_library_versioned_name_format = "{}.{}.wasm",
            static_dep_runtime_ld_flags = [],
            static_pic_dep_runtime_ld_flags = [],
            independent_shlib_interface_linker_flags = [],
            static_library_extension = "a",
            type = LinkerType("gnu"),
            use_archiver_flags = True,
            is_pdb_generated = False,
        ),
        binary_utilities_info = BinaryUtilitiesInfo(
            bolt_msdk = None,
            dwp = None,
            nm = tool("llvm-nm"),
            objcopy = tool("llvm-objcopy"),
            ranlib = tool("llvm-ranlib"),
            strip = tool("llvm-strip"),
        ),
        header_mode = HeaderMode("symlink_tree_only"),
    )

wasi_cxx_toolchain = rule(
    impl = _wasi_cxx_toolchain_impl,
    attrs = {
        "sdk": attrs.exec_dep(providers = [DefaultInfo]),
        "_internal_tools": attrs.default_only(attrs.exec_dep(
            default = "prelude//cxx/tools:internal_tools",
            providers = [CxxInternalTools],
        )),
    },
    is_toolchain_rule = True,
)
