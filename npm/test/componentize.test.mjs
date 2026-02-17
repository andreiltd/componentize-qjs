import { describe, it, expect } from "vitest";
import { componentize, runCli } from "../index.js";
import { readFileSync, existsSync, unlinkSync } from "node:fs";
import { resolve, join } from "node:path";
import { tmpdir } from "node:os";
import { mkdtempSync, rmSync } from "node:fs";

const examplesDir = resolve(__dirname, "../../examples");
const TIMEOUT = 30_000;

function readExample(name) {
  return readFileSync(resolve(examplesDir, name), "utf-8");
}

describe("componentize", () => {
  it("produces a valid wasm component from hello example", async () => {
    const result = await componentize({
      witPath: resolve(examplesDir, "hello.wit"),
      jsSource: readExample("hello.js"),
    });

    expect(result).toBeDefined();
    expect(result.component).toBeInstanceOf(Buffer);
    expect(result.component.length).toBeGreaterThan(0);
    // WebAssembly magic number: \0asm
    expect(result.component[0]).toBe(0x00);
    expect(result.component[1]).toBe(0x61);
    expect(result.component[2]).toBe(0x73);
    expect(result.component[3]).toBe(0x6d);
  }, TIMEOUT);

  it("produces a valid wasm component from math-provider example", async () => {
    const result = await componentize({
      witPath: resolve(examplesDir, "math-provider.wit"),
      jsSource: readExample("math-provider.js"),
    });

    expect(result.component).toBeInstanceOf(Buffer);
    expect(result.component.length).toBeGreaterThan(0);
  }, TIMEOUT);

  it("accepts an explicit world name", async () => {
    const result = await componentize({
      witPath: resolve(examplesDir, "hello.wit"),
      jsSource: readExample("hello.js"),
      world: "hello",
    });

    expect(result.component).toBeInstanceOf(Buffer);
    expect(result.component.length).toBeGreaterThan(0);
  }, TIMEOUT);

  it("produces a component with stub-wasi", async () => {
    const result = await componentize({
      witPath: resolve(examplesDir, "hello.wit"),
      jsSource: readExample("hello.js"),
      stubWasi: true,
    });

    expect(result.component).toBeInstanceOf(Buffer);
    expect(result.component.length).toBeGreaterThan(0);
  }, TIMEOUT);

  it("rejects a non-existent WIT path", async () => {
    await expect(
      componentize({
        witPath: "/nonexistent/path.wit",
        jsSource: 'function greet() { return "hi"; }',
      }),
    ).rejects.toThrow(/not found/i);
  });

  it("rejects an invalid world name", async () => {
    await expect(
      componentize({
        witPath: resolve(examplesDir, "hello.wit"),
        jsSource: readExample("hello.js"),
        world: "nonexistent-world",
      }),
    ).rejects.toThrow();
  });

  it("works with inline WIT and JS source", async () => {
    const { writeFileSync, mkdtempSync, rmSync } = await import("node:fs");
    const { join } = await import("node:path");
    const { tmpdir } = await import("node:os");

    const dir = mkdtempSync(join(tmpdir(), "cqjs-test-"));
    try {
      const witPath = join(dir, "test.wit");
      writeFileSync(
        witPath,
        `
        package test:inline;
        world inline {
          export add: func(a: u32, b: u32) -> u32;
        }
      `,
      );

      const result = await componentize({
        witPath,
        jsSource: "function add(a, b) { return a + b; }",
      });

      expect(result.component).toBeInstanceOf(Buffer);
      expect(result.component.length).toBeGreaterThan(0);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }, TIMEOUT);
});

describe("runCli", () => {
  it("produces a wasm file via CLI args", async () => {
    const dir = mkdtempSync(join(tmpdir(), "cqjs-cli-"));
    const output = join(dir, "out.wasm");
    try {
      const success = await runCli([
        "--wit", resolve(examplesDir, "hello.wit"),
        "--js", resolve(examplesDir, "hello.js"),
        "-o", output,
      ]);

      expect(success).toBe(true);
      expect(existsSync(output)).toBe(true);
      const wasm = readFileSync(output);
      expect(wasm[0]).toBe(0x00);
      expect(wasm[1]).toBe(0x61);
      expect(wasm[2]).toBe(0x73);
      expect(wasm[3]).toBe(0x6d);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }, TIMEOUT);

  it("returns false for missing required args", async () => {
    const success = await runCli([]);
    expect(success).toBe(false);
  });

  it("returns true for --help", async () => {
    const success = await runCli(["--help"]);
    expect(success).toBe(true);
  });
});
