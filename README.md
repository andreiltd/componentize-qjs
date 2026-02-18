# componentize-js but worse

This is largely based on https://github.com/alexcrichton/lua-component-demo and https://github.com/dicej/componentize-js

## Usage

```bash
# Build a component
cargo +nightly run --release -- --wit examples/hello.wit --js examples/hello.js -o hello.wasm

# Run with wasmtime
wasmtime run --invoke 'greet("World")' hello.wasm
```
