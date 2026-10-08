# componentize-qjs

[![CI](https://github.com/andreiltd/componentize-qjs/actions/workflows/buck2.yml/badge.svg)](https://github.com/andreiltd/componentize-qjs/actions/workflows/buck2.yml)
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

```bash
npm install componentize-qjs
```

This pulls in the right prebuilt native binding for your platform via the
`@andreiltd/componentize-qjs-binding-*` optional dependencies.

If you want to build from source run:

```bash
cd npm && npm install && npm run build
```

## Building with Buck2

Cargo remains supported and is still the source of truth for dependencies.
Buck2 builds the Rust crates, native dependencies, and all four embedded
`wasm32-wasip2` runtimes directly; it does not run Cargo or consume Cargo's
prebuilt runtimes. The `buck2` and `reindeer` launchers pin Buck2 2026-06-15
and Reindeer v2026.04.27.00, verifying downloaded binaries with
[DotSlash](https://dotslash-cli.com/). Buck downloads checksum-pinned Rust
1.99.0 (including Clippy and both standard libraries), native LLVM, Node
24.13.1, WASI SDK 33.0, and Binaryen 130 as declared build inputs.

Install DotSlash, Python 3 for Buck's bundled prelude tools, and your
platform's native C/C++ development libraries:
the libc development files on Linux, Xcode Command Line Tools on macOS, or
an x64 MSVC developer shell with the Windows SDK on Windows. Linux and
macOS support x86-64 and arm64; Windows supports x86-64. No configuration
helper, installed Rust/Clang/Node, or `rustup target add` is needed to build:

```bash
./buck2 build //:componentize-qjs
./buck2 run //:componentize-qjs -- --wit hello.wit --js hello.js -o hello.wasm
```

On Windows, invoke the pinned launchers as `dotslash buck2 ...` and
`dotslash reindeer ...`. The first build downloads several GiB of compiler
archives, then reuses unchanged locally materialized build outputs. Native LLVM
is 21.1.8 except on Intel macOS, which uses 19.1.7. Compiler inputs are tracked by Buck; native
OS libraries and SDKs are still supplied by the host, so the build is not
fully hermetic. On macOS, `xcrun` selects the native SDK for managed Clang
and Cargo build scripts, including libclang's header lookup.

| Target | Output |
| --- | --- |
| `//:componentize-qjs` | Native CLI |
| `//:componentize-qjs-lib` | Core Rust library |
| `//:componentize-qjs-node` | Node.js binding (`componentize-qjs.node`) |
| `//:runtimes` | All four runtime Wasm modules |
| `//:runtime`, `//:runtime-opt-size` | Async speed/size runtimes |
| `//:runtime-sync`, `//:runtime-opt-size-sync` | Sync speed/size runtimes |

Use `--show-full-simple-output` with `buck2 build` to print artifact paths.
The Node binding can be loaded directly with `require()` using its artifact
path; Cargo/npm packaging commands are unchanged.

```bash
./buck2 test --target-platforms root//platforms:test //:node-test
```

To exercise custom runtimes, including release-profile outputs, pass a
`//:runtimes` artifact directory to `./buck2 run //:node-smoke -- <directory>`.
The smoke test uses Buck's declared Node interpreter.

On memory-constrained hosts, add `-j 1` to limit simultaneous build actions:

```bash
./buck2 test -j 1 --target-platforms root//platforms:test //:
```

```bash
./buck2 test --target-platforms root//platforms:test //:

./buck2 build --target-platforms root//platforms:release //:componentize-qjs //:runtimes
./buck2 build -c componentize_qjs.async_support=false //:componentize-qjs
./buck2 build -c componentize_qjs.opt_size=true //:componentize-qjs
```

Profiles are target configurations: `root//platforms:dev` (default),
`root//platforms:test`, and `root//platforms:release`. Select one with
`--target-platforms`; the old `componentize_qjs.profile` flag is rejected.
Their artifacts use distinct configuration paths instead of overwriting
each other when switching profiles. The test profile optimizes dependencies
while retaining debug assertions, overflow checks, and the CLI's Cargo
test-profile optimization override for its library, executable, and test
suites. Release uses fat LTO and runs the pinned `wasm-opt` with the same
speed/size passes as Cargo. The two boolean
settings correspond to disabling `component-model-async` and enabling
`opt-size`; runtime-selection CLI options remain unchanged. `//:xtask`
builds the existing preparation tool, whose explicit execution still uses
Cargo; the Buck runtime graph does not depend on it.

### Command shortcuts

With [Just](https://just.systems/) installed, run `just` to list useful Buck
commands:

```bash
just build
just run --wit hello.wit --js hello.js -o hello.wasm
just test
just test //:cli-test
just clippy
just runtimes release
just test-node
just check
just check-runtimes
```

`just test` runs the root package's nine Rust suites and Node smoke test in
the optimized test profile. Generated crate downloads are not tests.
Bare `./buck2 test` still requires a target
pattern; use `//:` or `//...`. `just release` builds release artifacts, and
`just buckify` invokes Reindeer directly. `just clippy` lints all first-party
crates with all features through the managed Clippy driver and fails on
warnings or errors in Buck's diagnostic outputs. `just check` and
`just check-generated` check only the generated dependencies.
`just check-runtimes` exercises optimized runtimes and the sync-only,
size-default CLI. The recipes use a POSIX shell
(for example, Git Bash on Windows).

### Build-file layout

The root `BUCK` contains handwritten first-party targets. Like
[`cxx`](https://github.com/dtolnay/cxx), it loads the Cargo manifests directly
for package names, versions, editions, and compile-time environment metadata.
Reindeer generates only external dependencies in `third-party/BUCK`.
Supporting rules live beside the area they build:

| File | Responsibility |
| --- | --- |
| `BUCK` | Handwritten first-party sources and dependencies, with Cargo-derived metadata |
| `buck/config.bzl` | Profile attribute selection and feature switches |
| `buck/project.bzl` | Cargo metadata and explicit core/runtime/CLI/NAPI/xtask macros |
| `buck/runtime.bzl` | Wasm transitions, runtime optimization, and embedding |
| `platforms/defs.bzl` | Host execution platform and optional remote caching |
| `platforms/BUCK` | Host-based profile platforms and runtime optimization constraints |
| `toolchains/BUCK`, `toolchains/defs.bzl`, `toolchains/releases.bzl` | Managed native/WASI compilers and release pins |
| `third-party/defs.bzl` | Reindeer build scripts and crate downloads |
| `third-party/BUCK` | Generated external Rust dependencies |
| `tests/defs.bzl`, `tests/node.cjs` | Existing integration suites and Node smoke |
| `third-party/fixups/` | Reindeer package-specific build adjustments |

Only `third-party/BUCK` and `third-party/Cargo.lock` are generated.
Version-only release bumps are read directly from Cargo and do not require
regenerating the dependency graph. Cargo/release-plz remain responsible for
publishing.
Archive rules discard compilation-target constraints so native compilers and
crate sources are shared across profiles and runtime variants rather than
downloaded repeatedly.
BuildBuddy's endpoint configuration remains in `.github/buildbuddy.buckconfig`.

### Updating Buck dependencies

`third-party/Cargo.toml` lists the workspace's external normal, build, and
test dependency roots. Keep their version requirements and combined features
aligned with the
workspace manifests. Update the handwritten first-party dependency labels
in `BUCK` when dependencies change.
Regeneration requires installed Cargo/Rust 1.94+ and access to Cargo's
package cache or registries; ordinary Buck builds require neither.

After changing external dependencies or `Cargo.lock`, seed the import
lockfile from Cargo's lockfile and resolve the small import package:

```bash
cp Cargo.lock third-party/Cargo.lock
cargo metadata --format-version=1 \
  --manifest-path third-party/Cargo.toml > /dev/null
./reindeer --cargo-options=--locked buckify
just check
```

No nightly Cargo options or `RUSTC_BOOTSTRAP` are needed. `just check`
compares Reindeer's output with `third-party/BUCK`. Edit `third-party/fixups/`,
not generated rules, and include the generated files with dependency changes.

### Optional BuildBuddy cache

BuildBuddy is opt-in and runs actions **locally**, using its remote action
cache and CAS rather than remote execution:

```bash
export BUILDBUDDY_API_KEY=... # supply your key through your shell/secret manager
export BUCK2_TEST_FORCE_CACHE_UPLOAD=1 # testing-only override; see below
./buck2 build --config-file .github/buildbuddy.buckconfig \
  -c buck2_re_client.instance_name=componentize-qjs-linux-x86_64-ubuntu24.04-sdk1 \
  //:componentize-qjs
```

The key is expanded from the environment; its value is never written to the
configuration. `.github/buildbuddy.buckconfig` supplies the cache endpoints,
and cache uploads are enabled. An explicit instance namespace is required:
choose one identifying your host OS, architecture, native libraries, and SDK
version, and change it when those host inputs change. Managed compiler
changes invalidate actions automatically. Omit the cache configuration
flag for local-only builds; no generated local configuration is required.

The pinned prelude disables uploads for Rust actions by default. Cache-enabled
CI deliberately uses Buck's testing-only `BUCK2_TEST_FORCE_CACHE_UPLOAD`
override. When updating the Buck2 pin, recheck that switch's support and
verify Rust cache uploads and hits before relying on the cache. The override
also uploads extracted compiler archives, so measure CAS volume and transfer
time for each new runner-image namespace. Authenticated caching has not been
validated locally.

Pull requests use the Buck2 workflow's shared Just recipes for native builds,
tests, managed Clippy, generated dependencies, and runtime validation.
Cargo is used only for graph generation and Rust formatting in that workflow.
Release entry points reuse `.github/workflows/ci.yml` to require both Buck2
and Cargo builds, tests, and Clippy before publishing; published artifacts
remain Cargo-produced. Validation and publishing check out the same revision.
Each release entry point validates independently, including manual runs.
The Buck2 workflow uses the optional `BUILDBUDDY_API_KEY` secret only for
trusted pushes to the
protected `buildbuddy` environment; pull requests build without that
secret. Its cache namespace also includes the hosted runner's image version.
Configuring that environment and secret is optional.

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

The built-in runtime published with componentize-qjs includes component-model
async support. Pass `--sync` to embed the built-in non-async runtime instead,
producing components that run on hosts without component-model async support. A
custom runtime can also be supplied with `--runtime`.

## CLI Reference

```
componentize-qjs [OPTIONS] --wit <WIT> --js <JS>
```

| Flag | Short | Description |
|---|---|---|
| `--wit <PATH>` | `-w` | Path to the WIT file or directory |
| `--js <PATH>` | `-j` | Path to the JavaScript source file |
| `--output <PATH>` | `-o` | Output path (default: `output.wasm`) |
| `--module-root <PATH>` | | Root directory exposed read-only during Wizer for resolving JavaScript imports |
| `--world <NAME>` | `-n` | World name when the WIT defines multiple worlds |
| `--stub-wasi` | | Replace all WASI imports with trap stubs |
| `--minify` | `-m` | Minify JS source before embedding |
| `--opt-size` | | Use the built-in QuickJS runtime optimized for size |
| `--sync` | | Use the built-in non-async runtime (combine with `--opt-size` for the non-async opt-size runtime) |
| `--runtime <PATH>` | | Custom QuickJS runtime Wasm module to embed |

### Cargo features

| Feature | Effect |
|---|---|
| `component-model-async` | (default) Embed the component-model async runtime as the default built-in. The non-async runtime is always embedded and selectable via `--sync`. Disable to build a smaller binary with only the non-async runtime |
| `opt-size` | Selects the bundled opt-size runtime when no runtime option is provided by the CLI or npm API |

Build with features:

```bash
cargo build --release --features opt-size
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

JavaScript modules imported by the entry file are resolved during Wizer
initialization. Relative imports are resolved from the entry file path passed to
`--js`; bare package imports are resolved under the read-only module root. By
default the CLI uses the current directory when the entry file is under it, or
the entry file's parent directory otherwise. Use `--module-root <PATH>` to expose
a project root that contains shared files or `node_modules`.

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
| `map<K, V>` | `Map` | `new Map([["key", value]])` |
| `tuple<T, U, ...>` | `Array` | `[42, "hello"]` |
| `option<T>` | `T \| null` (nested: `{ tag: "some"\|"none", val }`) | `null` for none; `option<option<T>>` is wrapped |
| `result<T, E>` | top-level function result: return `T` or throw `E`; nested result: `{ tag: "ok"\|"err", val?: T\|E }` | `return 42` / `throw "error"` |
| `record { ... }` | `object` (camelCase keys) | `{ myField: 1 }` |
| `variant` | `{ tag: string, val?: T }` | `{ tag: "circle", val: 2.5 }` |
| `enum` | `string` (case name) | `"red"` |
| `flags` | `object` (camelCase booleans) | `{ read: true, write: false }` |
| `own<R>`, `borrow<R>` | resource object (methods on its prototype) | `input.blockingRead(n)` |

### Imported Resources

Imported resources are exposed as JavaScript classes. Resource methods are
called on the handle:

```js
import stdin from "wasi:cli/stdin@0.2.12";
import stdout from "wasi:cli/stdout@0.2.12";

const input = stdin.getStdin();     // an InputStream
const output = stdout.getStdout();  // an OutputStream

// Methods whose WIT return type is result<...> return the ok payload or throw.
const chunk = input.blockingRead(4096);   // method on the resource (len is a number)
output.blockingWriteAndFlush(chunk);
```

`[static]` methods are exposed on the resource class and `[constructor]` makes
the class callable with `new`.

Owned imported resources support `[Symbol.dispose]()` for deterministic
cleanup. Passing one to a WIT `own<T>` parameter transfers ownership and
invalidates the original wrapper. Borrowed wrappers expire when their component
call completes and cannot be transferred or disposed as owners. Resources lent
to an in-flight import remain alive and cannot be disposed or transferred until
that import completes. Ownership is restored for unconsumed stream/future writes
and for imported calls cancelled before they start. Await imports using an
incoming borrowed resource before returning from its export.

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

Exported resource methods and static methods may also be async. Implement them
as `async` members on the exported JavaScript class; resource constructors
remain synchronous:

```wit
package example:counter;

interface counter-api {
    resource counter {
        constructor(initial: u32);
        add: async func(value: u32) -> u32;
        create: static async func(initial: u32) -> counter;
    }
}

world counter {
    export counter-api;
}
```

```js
class Counter {
    constructor(initial) { this.value = initial; }
    async add(value) { this.value += value; return this.value; }
    static async create(initial) { return new this(initial); }
}

export const counterApi = { Counter };
```

### Streams

Streams transfer a sequence of values between components. Lifted WIT streams
implement the JavaScript async-iterable protocol, and async or synchronous
iterables are lowered to WIT streams automatically. The stream type is inferred
from the function parameter or result:

```wit
package example:streaming;

world streaming {
    export uppercase: async func(input: stream<string>) -> stream<string>;
}
```

```js
export async function uppercase(input) {
    return (async function* () {
        for await (const value of input) {
            yield value.toUpperCase();
        }
    })();
}
```

Async iteration yields one element at a time, except for `stream<u8>`, which
yields bounded `Uint8Array` chunks to avoid one promise per byte.

Use the `wit.Stream` factory when direct access to both endpoints is needed. If
only one stream type exists in the WIT world, the type may be omitted:

```js
const { readable, writable } = wit.Stream(wit.Stream.U8);
await writable.writeAll(new Uint8Array([1, 2, 3]));
writable.drop();
```

`wit.Stream.from` adapts an iterable explicitly and exposes a completion
promise:

```js
const source = (async function* () {
    yield "one";
    yield "two";
})();
const { readable, completion } = wit.Stream.from(source, wit.Stream.STRING);
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

Explicit WIT stream aliases are also exposed as constants:

```wit
type prompt-stream = stream<message-chunk>;
```

```js
wit.Stream(wit.Stream.PROMPT_STREAM);
```

Use `wit.Stream.types` or `wit.Future.types` to discover all available type
constants at runtime.

**StreamReadable methods:**

| Method | Returns | Description |
|--------|---------|-------------|
| `read(count?)` | `Promise<T[]>` (or `Uint8Array` for `u8`) | Read up to `count` values |
| `next()` | `Promise<IteratorResult<T>>` | Read the next value (`Uint8Array` chunk for `u8`) |
| `return()` | `Promise<IteratorResult<T>>` | End iteration and release the readable handle |
| `cancelRead()` | result or `undefined` | Cancel an in-progress read |
| `drop()` | `void` | Release the stream handle |
| `[Symbol.asyncIterator]()` | `StreamReadable` | Consume the stream with `for await...of` |

**StreamWritable methods:**

| Method | Returns | Description |
|--------|---------|-------------|
| `write(data)` | `Promise<number>` | Write values, returns count written |
| `writeOne(value)` | `Promise<number>` | Write exactly one value, including array-shaped WIT values |
| `writeAll(data)` | `Promise<number>` | Write all values, retrying as needed |
| `writeIterableItem(value)` | `Promise<boolean>` | Write one iterable yield, batching matching numeric typed arrays |
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
export async function compute() {
    const { readable, writable } = wit.Future();

    // Write the value (fire-and-forget; completes when reader reads)
    writable.write("computed result");

    return readable;
}
```

**Future type constants** follow the same pattern: `wit.Future.U32`,
`wit.Future.STRING`, etc.

Future handles are deliberately not thenable: use `await readable.read()` to
obtain the payload. This keeps `return readable` in an async export from
implicitly consuming the future, and preserves nested future handles.
Repeated `read()` calls share the same Promise; a cancelled read rejects that
Promise and allows a subsequent read to retry.

`wit.Future.from(value, type)` adapts a value or Promise and returns
`{ readable, completion }`. Return its `readable` from an async export rather
than returning the payload Promise directly, which JavaScript would await.

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

Owned imported resources, stream endpoints, and future endpoints support
[Explicit Resource Management](https://github.com/tc39/proposal-explicit-resource-management)
via `Symbol.dispose`. In environments that support `using`:

```js
{
    const stream = wit.Stream();
    using writable = stream.writable;
    using readable = stream.readable;
    // Each endpoint is disposed when leaving scope.
}
```

The factory's `{ readable, writable }` pair is not itself disposable.
Complete an endpoint's pending operation (or cancel it and await its
settlement) before leaving its `using` scope or calling `.drop()`.
Host-requested task cancellation retains pending ABI buffers until the host
acknowledges each operation's completion or cancellation.

Otherwise, call `.drop()` explicitly to release handles.

## Node.js API

The npm package exposes both a CLI and a programmatic API.

### CLI

```bash
npx componentize-qjs --wit hello.wit --js hello.js -o hello.wasm
```

(Or, if you installed the package globally, just `componentize-qjs ...`.)

### Usage

```js
import { componentize } from "componentize-qjs";

const { component } = await componentize({
    witPath: "hello.wit",
    jsSource: "export function greet(name) { return `Hello, ${name}!`; }",
    optSize: true,
});
// component is a Buffer containing the WebAssembly component bytes
```

Runtime selection is available through `optSize`, `sync`, `runtime`, or
`runtimeBytes`. `optSize` and `sync` may be combined to select the non-async
opt-size runtime, but neither can be combined with a custom `runtime`/`runtimeBytes`.
The `runtime` option is a path to a custom QuickJS runtime Wasm module.

## Acknowledgments

This project builds on ideas and code from:

- [ComponentizeJS](https://github.com/dicej/componentize-js) by Joel Dice
- [lua-component-demo](https://github.com/alexcrichton/lua-component-demo) by Alex Crichton

## License

Licensed under [Apache-2.0](LICENSE).
