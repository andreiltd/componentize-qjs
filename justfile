set positional-arguments

buck := "dotslash buck2"
reindeer := "dotslash reindeer"

# List available commands.
default:
    @just --list

# Build the CLI; accepts additional Buck flags.
build *args:
    {{ buck }} build //:componentize-qjs "$@"

# Run the CLI with its normal arguments.
run *args:
    {{ buck }} run //:componentize-qjs -- "$@"

# Run first-party tests in the optimized test profile, or select specific targets.
test +targets="//:":
    {{ buck }} test --target-platforms root//platforms:test "$@"

# Lint all first-party crates with all features; warnings and errors fail.
clippy *args:
    #!/usr/bin/env bash
    set -euo pipefail
    diagnostics="$({{ buck }} build --target-platforms root//platforms:test \
        -c componentize_qjs.async_support=true -c componentize_qjs.opt_size=true \
        --show-full-simple-output \
        '//:componentize-qjs-lib[clippy.json]' \
        '//:componentize-qjs-cli-lib[clippy.json]' \
        '//:componentize-qjs[clippy.json]' \
        '//:napi-lib[clippy.json]' \
        '//:runtime-sync-module[clippy.json]' \
        '//:runtime-async-module[clippy.json]' \
        '//:xtask[clippy.json]' "$@")"
    # Diagnostic subtargets are infallible, so inspect their reported levels.
    python3 - "$diagnostics" <<'PY'
    import json
    import sys

    paths = sys.argv[1].splitlines()

    if not paths:
        raise SystemExit("Buck produced no Clippy diagnostic files.")

    failed = False

    for path in paths:
        with open(path, encoding="utf-8") as report:
            for line in report:
                diagnostic = json.loads(line)

                if diagnostic.get("level") in ("warning", "error"):
                    print(diagnostic.get("rendered") or diagnostic["message"], file=sys.stderr)
                    failed = True

    sys.exit(1 if failed else 0)
    PY

# Build release CLI, Node binding, and all four optimized runtimes.
release *args:
    {{ buck }} build --target-platforms root//platforms:release //:componentize-qjs //:componentize-qjs-node //:runtimes "$@"

# Build all four runtimes; profile accepts dev, test, or release.
runtimes profile="test":
    {{ buck }} build --target-platforms "root//platforms:$1" //:runtimes

# Build the Node binding in the optimized test profile.
node:
    {{ buck }} build --target-platforms root//platforms:test //:componentize-qjs-node

# Exercise all four built-in runtimes through the Node binding.
test-node:
    {{ buck }} test --target-platforms root//platforms:test //:node-test

# Regenerate third-party/BUCK from the locked external dependency manifest.
buckify:
    {{ reindeer }} --cargo-options=--locked buckify

# Check that the generated third-party graph is current.
check-generated:
    #!/usr/bin/env bash
    set -euo pipefail
    {{ reindeer }} --cargo-options=--locked buckify --stdout | diff - third-party/BUCK

# Check that the generated dependency graph is current.
check: check-generated

# Exercise optimized runtimes and the sync-only CLI; accepts Buck build flags.
check-runtimes *args:
    #!/usr/bin/env bash
    set -euo pipefail
    runtimes="$({{ buck }} build --target-platforms root//platforms:release --show-full-simple-output //:runtimes "$@")"
    {{ buck }} run --target-platforms root//platforms:test "$@" //:node-smoke -- "$runtimes"
    cli="$({{ buck }} build --target-platforms root//platforms:test -c componentize_qjs.async_support=false -c componentize_qjs.opt_size=true --show-full-simple-output //:componentize-qjs "$@")"
    output="$(mktemp)"
    trap 'rm -f "$output"' EXIT
    "$cli" --wit examples/hello.wit --js examples/hello.js -o "$output"
    if [[ "$(od -An -tx1 -N8 "$output" | tr -d ' \n')" != "0061736d0d000100" ]]; then
        printf 'The sync-only CLI did not produce a WebAssembly component.\n' >&2
        exit 1
    fi
