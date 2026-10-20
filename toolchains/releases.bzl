"""Checksum-pinned compilers, runtime tools, and test interpreters."""

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
