"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

async function main() {
    if (!process.argv[2]) {
        throw new Error("Usage: buck2 run //:node-smoke -- [runtime directory]");
    }

    const binding = require(path.resolve(process.argv[2]));
    const runtimes = process.argv[3]
        ? ["runtime.wasm", "runtime-sync.wasm", "runtime-opt-size.wasm", "runtime-opt-size-sync.wasm"]
            .map((filename) => ({ runtime: path.join(path.resolve(process.argv[3]), filename) }))
        : [{}, { sync: true }, { optSize: true }, { sync: true, optSize: true }];
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), "componentize-qjs-node-"));
    const witPath = path.join(directory, "test.wit");

    try {
        fs.writeFileSync(witPath, "package test:buck; world test { export add: func(a: u32, b: u32) -> u32; }");

        for (const runtime of runtimes) {
            const { component } = await binding.componentize({
                witPath,
                jsSource: "export function add(a, b) { return a + b; }",
                ...runtime,
            });

            assert(Buffer.isBuffer(component));
            assert.deepEqual(component.subarray(0, 8), Buffer.from([0, 97, 115, 109, 13, 0, 1, 0]));
        }
    } finally {
        fs.unlinkSync(witPath);
        fs.rmdirSync(directory);
    }
}

main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
});
