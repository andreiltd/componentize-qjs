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

Building currently requires a **nightly** Rust toolchain because it relies on the wasi-libc being compiled with `-fPIC`. This requirement will be lifted with an upcoming version of Rust.

## Installation

### Rust CLI (from source)

```bash
cargo +nightly install --path .
```

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

```js
function greet(name) {
    return `Hello, ${name}!`;
}
```

**3. Build the component:**

```bash
componentize-qjs --wit hello.wit --js hello.js -o hello.wasm
```

**4. Run it:**

```bash
wasmtime run --invoke 'greet("World")' hello.wasm
# Hello, World!
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
cargo +nightly build --release --features optimize-size
```

## Using Imports

WIT imports are available on `globalThis` keyed by their fully-qualified name:

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
function doubleAdd(a, b) {
    const math = globalThis["local:test/math"];
    const sum = math.add(a, b);
    return math.multiply(sum, 2);
}
```

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
    jsSource: "function greet(name) { return `Hello, ${name}!`; }",
});
// component is a Buffer containing the WebAssembly component bytes
```

## Acknowledgments

This project builds on ideas and code from:

- [ComponentizeJS](https://github.com/bytecodealliance/ComponentizeJS) by Joel Dice
- [lua-component-demo](https://github.com/alexcrichton/lua-component-demo) by Alex Crichton

## License

Licensed under [Apache-2.0](LICENSE).
