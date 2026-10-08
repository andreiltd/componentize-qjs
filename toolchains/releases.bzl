"""Checksum-pinned compilers, runtime tools, and test interpreters."""

RUST_VERSION = "1.99.0"
RUST_DATE = "2026-10-01"
RUSTC_SHA256 = {
    "aarch64-apple-darwin": "334a66714ca316d71bbe5efe44f71762d0b276cea73be2987374693a837f7450",
    "aarch64-unknown-linux-gnu": "89c0f1a3a44df63c95e5d5f2ba093d0a12c5f5f57f8635f8a7a0ce50cf86600e",
    "x86_64-apple-darwin": "7460910254f059b49b99ee3ddbcf7197d16b9505b59c166e14a1465ee323a81d",
    "x86_64-pc-windows-msvc": "20fed82e629c1ed145f9e9bdee9aad65e790d4237a49bcfc61221eda69e35b14",
    "x86_64-unknown-linux-gnu": "77171ba2a0345fdf2abc4fedda55d6de078dae7a68527c28be8c77dcc9604bd5",
}
CLIPPY_SHA256 = {
    "aarch64-apple-darwin": "c31a0765ed31d98515a09a13b44486048873d90ac75bb1e6690a17fb36a5d4e2",
    "aarch64-unknown-linux-gnu": "42e49dc02d29949395c0c072585961f6c9b5408110236087828d4e2f94ca96ef",
    "x86_64-apple-darwin": "cd93518193f16b21cfa7147a42cb03cd2d9ca9b3d0c2daf46dcae44c88a696df",
    "x86_64-pc-windows-msvc": "b1b94d7d7717f42727eec41e70952100d865d25ebb7b1dd08041f6971237cfa9",
    "x86_64-unknown-linux-gnu": "982442e32ad8dd3f0bb3a30db74038da5f71fd9bf1a865cc71da8ea5d4ec71ec",
}
RUST_STD_SHA256 = {
    "aarch64-apple-darwin": "a7c6de8aa21e7c31163a7656295b321bb85f5dce55ed8ea95ac9ac561202bb5b",
    "aarch64-unknown-linux-gnu": "1cf7e2ef58ed1cfa6f1adea18e155cbf254c1951f39e5f5e3376879f55fd64a4",
    "wasm32-wasip2": "5dc556f4080f40af9b825d2e1409d284d0dae59443a03e939c30b9acf7692b71",
    "x86_64-apple-darwin": "18ee81961a73c3ae966ecb1e8f6096438c90aeefece8856201a6ecc17f5bc850",
    "x86_64-pc-windows-msvc": "adadeafff137a7610884696912b853d4de259c4299ab035b66661e2e0f923ff0",
    "x86_64-unknown-linux-gnu": "3e58dff2d0b72196b5ea4e90536e174d400de88564a52694686b81e091169933",
}

# LLVM no longer publishes Intel macOS binaries in the newer release series.
LLVM_RELEASES = {
    "arm64-linux": ("21.1.8", "LLVM-21.1.8-Linux-ARM64", "65ce0b329514e5643407db2d02a5bd34bf33d159055dafa82825c8385bd01993"),
    "arm64-macos": ("21.1.8", "LLVM-21.1.8-macOS-ARM64", "b95bdd32a33a81ee4d40363aaeb26728a26783fcef26a4d80f65457433ea4669"),
    "x86_64-linux": ("21.1.8", "LLVM-21.1.8-Linux-X64", "b3b7f2801d15d50736acea3c73982994d025b01c2f035b91ae3b49d1b575732b"),
    "x86_64-macos": ("19.1.7", "LLVM-19.1.7-macOS-X64", "49405e75fbe7ad6f8139a33f59ec8c5112b75b3027405c7b92d19f4c6f02c78a"),
    "x86_64-windows": ("21.1.8", "clang+llvm-21.1.8-x86_64-pc-windows-msvc", "749d22f565fcd5718dbed06512572d0e5353b502c03fe1f7f17ee8b8aca21a47"),
}

NODE_VERSION = "24.13.1"
NODE_SHA256 = {
    "darwin-arm64": "d82a321541d65109c696505135be3b7dd46e3358f0f04d664f50f0d1e1ccb8a6",
    "darwin-x64": "013a8f786a022ad1729cf435e3675e097a77d5a42eaf139a2d5d1d5309a027d4",
    "linux-arm64": "c827d3d301e2eed1a51f36d0116b71b9e3d9e3b728f081615270ea40faac34c1",
    "linux-x64": "30215f90ea3cd04dfbc06e762c021393fa173a1d392974298bbc871a8e461089",
    "win-x64": "fba577c4bb87df04d54dd87bbdaa5a2272f1f99a2acbf9152e1a91b8b5f0b279",
}

WASI_SDK_VERSION = "33.0"
WASI_SDK_SHA256 = {
    "arm64-linux": "4f98ee738c7abb45c81a94d1461fc53cc569d1cd01498951c8184d841a027844",
    "arm64-macos": "85c997a2665ead91673b5bb88b7d0df3fc8900df3bfa244f720d478187bbdc78",
    "arm64-windows": "2f457a62da1ce1a55e2ba77c450401b3551f27f04f0a87112b74c5aa8dd9504f",
    "x86_64-linux": "0ba8b5bfaeb2adf3f29bab5841d76cf5318ab8e1642ea195f88baba1abd47bce",
    "x86_64-macos": "18f3f201ba9734e6a4455b0b6410690395a55e9ffa9f6f5066f66083a94b93b3",
    "x86_64-windows": "df14ca2a2127c2d6b6be07e6f5549b3af9c1b3c0112430c200a4749970c59f06",
}

BINARYEN_VERSION = "130"
BINARYEN_SHA256 = {
    "aarch64-linux": "e6ae6e09ac40f4e14bc5be6f687c58e2995c84170013975fa641809dd3b480a0",
    "arm64-macos": "79d3ab9f417d9e215f15f598f523d001a7d9ac1e59367e5c869fbdabd1cba72e",
    "x86_64-linux": "0a18362361ad05465118cd8eeb72edaeec89de6894bc283576ef4e07aa3babcc",
    "x86_64-macos": "d3e2d1235b70c93c54b52eabc1625ea960965152218754f1f4eeb0f873c48e03",
    "x86_64-windows": "cc09c874f4332d00aa32ab72745a9b98c9a172f795762f21d03e70638a3f7f4c",
}
