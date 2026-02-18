# componentize-js but worse

The runtime is based on `quickjs` through `rquickjs`, a small embeddable JavaScript engine.

This is largely based on https://github.com/alexcrichton/lua-component-demo and https://github.com/dicej/componentize-js

## Prerequisites

Building currently requires a **nightly** Rust toolchain because it relies on the wasi-libc being compiled with `-fPIC`.

## Usage

```bash
# Build a component
cargo +nightly run --release -- --wit examples/hello.wit --js examples/hello.js -o hello.wasm

# Build with optimize-size feature for smaller output (~750KB for the hello component)
cargo +nightly run --release --features optimize-size -- --wit examples/hello.wit --js examples/hello.js -o hello.wasm

# Run with wasmtime
wasmtime run --invoke 'greet("World")' hello.wasm
```
