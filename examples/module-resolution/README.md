# Module resolution example

This example shows JavaScript imports resolved during Wizer initialization:

- `./lib/math.js` is a relative import.
- `./config` demonstrates extension inference.
- `local-greeter` is resolved from the local `node_modules` fixture.

Build it from the repository root:

```bash
componentize-qjs \
  --wit examples/module-resolution/package.wit \
  --js examples/module-resolution/main.js \
  --module-root examples/module-resolution \
  --output module-resolution.wasm
```

The `--module-root` directory is exposed read-only during Wizer so imported
files can be read and baked into the generated component.

The runtime loader expects ES modules (`.js` or `.mjs`). It resolves package
metadata with `oxc_resolver`, but it does not transform CommonJS packages.
