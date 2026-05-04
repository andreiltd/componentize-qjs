# componentize-qjs

[![CI](https://github.com/andreiltd/componentize-qjs/actions/workflows/ci.yml/badge.svg)](https://github.com/andreiltd/componentize-qjs/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

Convert JavaScript source code into
[WebAssembly components](https://component-model.bytecodealliance.org/) using
[QuickJS](https://github.com/quickjs-ng/quickjs).

## Overview

`componentize-qjs` takes a JavaScript source file and a
[WIT](https://component-model.bytecodealliance.org/design/wit.html) definition,
and produces a standalone WebAssembly component that can run on any
component-model runtime (e.g. [Wasmtime](https://wasmtime.dev/)).

Under the hood it:

1. Embeds the [QuickJS](https://github.com/quickjs-ng/quickjs) engine (via
   [rquickjs](https://github.com/DelSkayn/rquickjs)) as the JavaScript
   runtime.
2. Uses [wit-dylib](https://crates.io/crates/wit-dylib) to generate WIT
   bindings that bridge the component model and the JS engine.
3. Snapshots the initialized JS state with
   [Wizer](https://github.com/bytecodealliance/wizer) so startup cost is paid at
   build time, not at runtime.

## Prerequisites

Rust **1.94** or later is required (the `wasm32-wasip2` target needs a recent
toolchain for PIC support in wasi-libc).

## Installation

### Rust CLI (crates.io)

```bash
cargo install componentize-qjs-cli --locked
```

This installs the `componentize-qjs` command.

### Rust CLI (from source)

```bash
cargo install --path . --locked
```

### Prebuilt CLI binaries

Prebuilt CLI archives are attached to each
[GitHub release](https://github.com/andreiltd/componentize-qjs/releases) for
Linux, macOS, and Windows.

### npm package

The npm package is not yet published to the registry. To use it, build from source:

```bash
cd npm && npm install && npm run build
```

## Quick Start

**1. Define a WIT interface** (`hello.wit`):

```wit
package test:hello;

world hello {
    export greet: func(name: string) -> string;
}
```

**2. Implement it in JavaScript** (`hello.js`):

JavaScript sources are ES modules. Export WIT functions and interfaces directly
from the module.

```js
export function greet(name) {
    return `Hello, ${name}!`;
}
```

**3. Build the component:**

```bash
componentize-qjs --wit hello.wit --js hello.js -o hello.wasm
```

**4. Run it:**

```bash
wasmtime run --wasm component-model-async=y --invoke 'greet("World")' hello.wasm
# "Hello, World!"
```

## CLI Reference

```
componentize-qjs [OPTIONS] --wit <WIT> --js <JS>
```

| Flag | Short | Description |
|---|---|---|
| `--wit <PATH>` | `-w` | Path to the WIT file or directory |
| `--js <PATH>` | `-j` | Path to the JavaScript source file |
| `--output <PATH>` | `-o` | Output path (default: `output.wasm`) |
| `--world <NAME>` | `-n` | World name when the WIT defines multiple worlds |
| `--stub-wasi` | | Replace all WASI imports with trap stubs |
| `--minify` | `-m` | Minify JS source before embedding |

### Cargo features

| Feature | Effect |
|---|---|
| `optimize-size` | Uses `-Oz` instead of `-O3` for smaller output |

Build with features:

```bash
cargo build --release --features optimize-size
```

## Using Imports

WIT imports are available as ES module imports using their fully-qualified WIT
interface name:

```wit
// imports.wit
package local:test;

interface math {
    add: func(a: s32, b: s32) -> s32;
    multiply: func(a: s32, b: s32) -> s32;
}

world imports {
    import math;
    export double-add: func(a: s32, b: s32) -> s32;
}
```

```js
// imports.js
import math from "local:test/math";

export function doubleAdd(a, b) {
    const sum = math.add(a, b);
    return math.multiply(sum, 2);
}
```

## WIT Type Mappings

### Primitive Types

| WIT Type | JS Type | Notes |
|----------|---------|-------|
| `bool` | `boolean` | |
| `u8`, `u16`, `u32` | `number` | |
| `s8`, `s16`, `s32` | `number` | |
| `u64`, `s64` | `number` | Precision limited to 2⁵³ (Number.MAX_SAFE_INTEGER) |
| `f32`, `f64` | `number` | |
| `char` | `string` | Must be exactly one Unicode scalar value |
| `string` | `string` | |

### Compound Types

| WIT Type | JS Type | Example |
|----------|---------|---------|
| `list<T>` | `Array` | `[1, 2, 3]` |
| `list<u8>` | `Uint8Array` or `Array` | `new Uint8Array([1, 2, 3])` |
| `tuple<T, U, ...>` | `Array` | `[42, "hello"]` |
| `option<T>` | `T \| null \| undefined` | `null` for none |
| `result<T, E>` | `{ tag: "ok"\|"err", val?: T\|E }` | `{ tag: "ok", val: 42 }` |
| `record { ... }` | `object` (camelCase keys) | `{ myField: 1 }` |
| `variant` | `{ tag: number, val?: T }` | `{ tag: 0, val: "hi" }` |
| `enum` | `number` | Lookup tables provided on the interface |
| `flags` | `number` (bitmask) | Bit constants provided on the interface |
| `own<R>`, `borrow<R>` | `number` (handle) | Opaque resource handle |

### Async Exports

Async exports are declared with the `async` keyword in WIT and implemented
as JavaScript `async` functions:

```wit
package example:greeting;

world greeter {
    export greet: async func(name: string) -> string;
}
```

```js
export async function greet(name) {
    // You can use await here
    await Promise.resolve();
    return `Hello, ${name}!`;
}
```

### Streams

Streams transfer a sequence of values between components.
The `wit` global provides `Stream` and `Future` constructors for creating
stream/future pairs. The type is automatically determined from the WIT definition:

```wit
package example:streaming;

world streaming {
    export produce: async func() -> stream<u8>;
}
```

```js
async function produce() {
    // When only one stream type exists in the WIT, no argument needed
    const { readable, writable } = wit.Stream();

    writable.write(new Uint8Array([1, 2, 3]));
    writable.drop();

    return readable;
}
```

When the WIT defines multiple stream types, use the type constant:

```js
// wit.Stream.U8, wit.Stream.STRING, wit.Stream.U32, etc.
const { readable, writable } = wit.Stream(wit.Stream.U8);
const { readable, writable } = wit.Stream(wit.Stream.STRING);
```

Available type constants (populated from WIT metadata):

| WIT type | Constant |
|----------|----------|
| `stream<u8>` / `future<u8>` | `wit.Stream.U8` / `wit.Future.U8` |
| `stream<u32>` / `future<u32>` | `wit.Stream.U32` / `wit.Future.U32` |
| `stream<string>` / `future<string>` | `wit.Stream.STRING` / `wit.Future.STRING` |
| `stream<f64>` / `future<f64>` | `wit.Stream.F64` / `wit.Future.F64` |

All constructors return `{ readable, writable }`.

**Complex element types** are also supported. The type constant is generated
recursively from the WIT type structure:

```js
// stream<result<string, u32>>
wit.Stream(wit.Stream.RESULT_STRING_U32);

// stream<option<u32>>
wit.Stream(wit.Stream.OPTION_U32);

// stream<tuple<u32, string>>
wit.Stream(wit.Stream.TUPLE_U32_STRING);

// Named record types use their WIT name:
// record point { x: f64, y: f64 }
// stream<point>
wit.Stream(wit.Stream.POINT);
```

Use `wit.Stream.types` or `wit.Future.types` to discover all available type
constants at runtime.

**StreamReadable methods:**

| Method | Returns | Description |
|--------|---------|-------------|
| `read(count?)` | `Promise<T[]>` (or `Uint8Array` for `u8`) | Read up to `count` values |
| `cancelRead()` | result or `undefined` | Cancel an in-progress read |
| `drop()` | `void` | Release the stream handle |

**StreamWritable methods:**

| Method | Returns | Description |
|--------|---------|-------------|
| `write(data)` | `Promise<number>` | Write values, returns count written |
| `writeAll(data)` | `Promise<number>` | Write all values, retrying as needed |
| `cancelWrite()` | result or `undefined` | Cancel an in-progress write |
| `drop()` | `void` | Release the stream handle |

### Futures

Futures transfer a single value. They work like streams but carry exactly one
value:

```wit
package example:async-value;

world async-value {
    export compute: async func() -> future<string>;
}
```

```js
async function compute() {
    const { readable, writable } = wit.Future();

    // Write the value (fire-and-forget; completes when reader reads)
    writable.write("computed result");

    return readable;
}
```

**Future type constants** follow the same pattern: `wit.Future.U32`,
`wit.Future.STRING`, etc.

**FutureReadable methods:**

| Method | Returns | Description |
|--------|---------|-------------|
| `read()` | `Promise<T>` | Read the single value |
| `cancelRead()` | result or `undefined` | Cancel an in-progress read |
| `drop()` | `void` | Release the future handle |

**FutureWritable methods:**

| Method | Returns | Description |
|--------|---------|-------------|
| `write(value)` | `Promise<boolean>` | Write the value, returns success |
| `cancelWrite()` | result or `undefined` | Cancel an in-progress write |
| `drop()` | `void` | Release the future handle |

### Resource Cleanup

Stream and future handles support
[Explicit Resource Management](https://github.com/tc39/proposal-explicit-resource-management)
via `Symbol.dispose`. In environments that support `using`:

```js
{
    using stream = wit.Stream();
    // stream.writable and stream.readable are auto-dropped when leaving scope
}
```

Otherwise, call `.drop()` explicitly to release handles.

## Node.js API

The npm package exposes both a CLI and a programmatic API. It is not yet
published to the registry — see [Installation](#installation) for building from
source.

### CLI

```bash
./npm/bin/componentize-qjs --wit hello.wit --js hello.js -o hello.wasm
```

### Usage

```js
import { componentize } from "componentize-qjs";

const { component } = await componentize({
    witPath: "hello.wit",
    jsSource: "export function greet(name) { return `Hello, ${name}!`; }",
});
// component is a Buffer containing the WebAssembly component bytes
```

## Acknowledgments

This project builds on ideas and code from:

- [ComponentizeJS](https://github.com/dicej/componentize-js) by Joel Dice
- [lua-component-demo](https://github.com/alexcrichton/lua-component-demo) by Alex Crichton

## License

Licensed under [Apache-2.0](LICENSE).
