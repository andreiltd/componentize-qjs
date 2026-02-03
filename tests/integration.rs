//! Integration tests for componentize-qjs
//!
//! These tests build components using the CLI and run them with wasmtime.

use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

/// Helper to create a test environment with WIT and JS files
struct TestEnv {
    dir: TempDir,
}

impl TestEnv {
    fn new() -> Self {
        Self {
            dir: TempDir::new().expect("Failed to create temp dir"),
        }
    }

    fn write_file(&self, name: &str, content: &str) -> std::path::PathBuf {
        let path = self.dir.path().join(name);
        fs::write(&path, content).expect("Failed to write file");
        path
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }
}

/// Get the componentize-qjs binary
#[allow(deprecated)]
fn componentize_qjs() -> assert_cmd::Command {
    assert_cmd::Command::cargo_bin("componentize-qjs").unwrap()
}

/// Run a component with wasmtime and invoke a function
fn run_component(wasm_path: &std::path::Path, dir: &std::path::Path, invoke: &str) -> String {
    run_component_with_args(wasm_path, dir, invoke, &[])
}

/// Run a component with wasmtime, invoke a function, and pass extra args
fn run_component_with_args(
    wasm_path: &std::path::Path,
    dir: &std::path::Path,
    invoke: &str,
    extra_args: &[&str],
) -> String {
    let mut cmd = std::process::Command::new("wasmtime");
    cmd.current_dir(dir).arg("run").arg("--dir").arg(".");

    for arg in extra_args {
        cmd.arg(arg);
    }

    cmd.arg("--invoke").arg(invoke).arg(wasm_path);

    let output = cmd.output().expect("Failed to run wasmtime");

    if !output.status.success() {
        panic!(
            "wasmtime failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn test_hello_world() {
    let env = TestEnv::new();

    let wit = env.write_file(
        "hello.wit",
        r#"
        package test:hello;

        world hello {
            export greet: func() -> string;
            export add: func(a: u32, b: u32) -> u32;
        }
        "#,
    );

    let js = env.write_file(
        "hello.js",
        r#"
        function greet() {
            return "Hello, World!";
        }

        function add(a, b) {
            return a + b;
        }
        "#,
    );

    // Also write as main.js for runtime to find
    env.write_file(
        "main.js",
        r#"
        function greet() {
            return "Hello, World!";
        }

        function add(a, b) {
            return a + b;
        }
        "#,
    );

    let output = env.path("hello.wasm");

    // Build the component
    componentize_qjs()
        .arg("--wit")
        .arg(&wit)
        .arg("--js")
        .arg(&js)
        .arg("--output")
        .arg(&output)
        .assert()
        .success()
        .stdout(predicate::str::contains("Component written to"));

    assert!(output.exists(), "Output wasm file should exist");

    // Run and verify
    let result = run_component(&output, env.dir.path(), "add(2, 3)");
    assert_eq!(result, "5");
}

#[test]
fn test_numeric_types() {
    let env = TestEnv::new();

    let wit = env.write_file(
        "types.wit",
        r#"
        package test:types;

        world types {
            export add-u32: func(a: u32, b: u32) -> u32;
            export add-s32: func(a: s32, b: s32) -> s32;
            export add-f64: func(a: f64, b: f64) -> f64;
            export negate: func(b: bool) -> bool;
        }
    "#,
    );

    let js_content = r#"
    function addU32(a, b) { return a + b; }
    function addS32(a, b) { return a + b; }
    function addF64(a, b) { return a + b; }
    function negate(b) { return !b; }
    "#;

    env.write_file("types.js", js_content);
    env.write_file("main.js", js_content);

    let output = env.path("types.wasm");

    componentize_qjs()
        .arg("--wit")
        .arg(&wit)
        .arg("--js")
        .arg(env.path("types.js"))
        .arg("-o")
        .arg(&output)
        .assert()
        .success();

    // Test u32
    assert_eq!(
        run_component(&output, env.dir.path(), "add-u32(100, 200)"),
        "300"
    );

    // Test s32 (negative numbers)
    assert_eq!(
        run_component(&output, env.dir.path(), "add-s32(-10, 5)"),
        "-5"
    );

    // Test f64
    assert_eq!(
        run_component(&output, env.dir.path(), "add-f64(1.5, 2.5)"),
        "4"
    );

    // Test bool
    assert_eq!(
        run_component(&output, env.dir.path(), "negate(true)"),
        "false"
    );
}

#[test]
fn test_record_type() {
    let env = TestEnv::new();

    let wit = env.write_file(
        "record.wit",
        r#"
        package test:records;

        world record-test {
            record point {
                x: f64,
                y: f64,
            }
            export add-points: func(a: point, b: point) -> point;
        }
        "#,
    );

    let js_content = r#"
    function addPoints(a, b) {
        return { x: a.x + b.x, y: a.y + b.y };
    }
    "#;

    env.write_file("record.js", js_content);
    env.write_file("main.js", js_content);

    let output = env.path("record.wasm");

    componentize_qjs()
        .arg("--wit")
        .arg(&wit)
        .arg("--js")
        .arg(env.path("record.js"))
        .arg("-o")
        .arg(&output)
        .assert()
        .success();

    // Test record - wasmtime uses {field: value} syntax
    let result = run_component(
        &output,
        env.dir.path(),
        "add-points({x: 1.0, y: 2.0}, {x: 3.0, y: 4.0})",
    );
    assert!(
        result.contains("4") && result.contains("6"),
        "Expected point with x=4, y=6, got: {}",
        result
    );
}

#[test]
fn test_list_type() {
    let env = TestEnv::new();

    let wit = env.write_file(
        "list.wit",
        r#"
        package test:lists;

        world list-test {
          export sum-list: func(nums: list<u32>) -> u32;
          export double-list: func(nums: list<u32>) -> list<u32>;
        }
        "#,
    );

    let js_content = r#"
    function sumList(nums) {
      return nums.reduce((a, b) => a + b, 0);
    }

    function doubleList(nums) {
      return nums.map(n => n * 2);
    }
    "#;

    env.write_file("list.js", js_content);
    env.write_file("main.js", js_content);

    let output = env.path("list.wasm");

    componentize_qjs()
        .arg("--wit")
        .arg(&wit)
        .arg("--js")
        .arg(env.path("list.js"))
        .arg("-o")
        .arg(&output)
        .assert()
        .success();

    // Test sum
    assert_eq!(
        run_component(&output, env.dir.path(), "sum-list([1, 2, 3, 4, 5])"),
        "15"
    );
}

#[test]
fn test_option_type() {
    let env = TestEnv::new();

    let wit = env.write_file(
        "option.wit",
        r#"
        package test:options;

        world option-test {
          export maybe-double: func(n: option<u32>) -> option<u32>;
        }
        "#,
    );

    let js_content = r#"
    function maybeDouble(n) {
      if (n === null || n === undefined) {
        return null;
      }
      return n * 2;
    }
    "#;

    env.write_file("option.js", js_content);
    env.write_file("main.js", js_content);

    let output = env.path("option.wasm");

    componentize_qjs()
        .arg("--wit")
        .arg(&wit)
        .arg("--js")
        .arg(env.path("option.js"))
        .arg("-o")
        .arg(&output)
        .assert()
        .success();

    // Test some
    let result = run_component(&output, env.dir.path(), "maybe-double(some(5))");
    assert!(result.contains("10"), "Expected some(10), got: {}", result);

    // Test none
    let result = run_component(&output, env.dir.path(), "maybe-double(none)");
    assert!(
        result.contains("none") || result.is_empty() || result == "none",
        "Expected none, got: {}",
        result
    );
}

#[test]
fn test_result_type() {
    let env = TestEnv::new();

    let wit = env.write_file(
        "result.wit",
        r#"
        package test:results;

        world result-test {
          export safe-div: func(a: u32, b: u32) -> result<u32, string>;
        }
       "#,
    );

    let js_content = r#"
    function safeDiv(a, b) {
      if (b === 0) {
        return { tag: "err", val: "division by zero" };
      }
      return { tag: "ok", val: Math.floor(a / b) };
    }
    "#;

    env.write_file("result.js", js_content);
    env.write_file("main.js", js_content);

    let output = env.path("result.wasm");

    componentize_qjs()
        .arg("--wit")
        .arg(&wit)
        .arg("--js")
        .arg(env.path("result.js"))
        .arg("-o")
        .arg(&output)
        .assert()
        .success();

    // Test ok
    let result = run_component(&output, env.dir.path(), "safe-div(10, 2)");
    assert!(
        result.contains("ok") && result.contains("5"),
        "Expected ok(5), got: {}",
        result
    );

    // Test err
    let result = run_component(&output, env.dir.path(), "safe-div(10, 0)");
    assert!(result.contains("err"), "Expected err, got: {}", result);
}

#[test]
fn test_cli_errors() {
    // Test missing WIT file
    componentize_qjs()
        .arg("--wit")
        .arg("nonexistent.wit")
        .arg("--js")
        .arg("test.js")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));

    // Test missing JS file
    let env = TestEnv::new();
    let wit = env.write_file("test.wit", "package test:test; world test {}");

    componentize_qjs()
        .arg("--wit")
        .arg(&wit)
        .arg("--js")
        .arg("nonexistent.js")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn test_stub_wasi() {
    let env = TestEnv::new();

    let wit = env.write_file(
        "hello.wit",
        r#"
        package test:hello;

        world hello {
            export greet: func(name: string) -> string;
            export add: func(a: u32, b: u32) -> u32;
        }
        "#,
    );

    let js = env.write_file(
        "hello.js",
        r#"
        function greet(name) {
            return "Hello, " + name + "!";
        }

        function add(a, b) {
            return a + b;
        }
        "#,
    );

    let output = env.path("hello-stubbed.wasm");

    // Build with --stub-wasi
    componentize_qjs()
        .arg("--wit")
        .arg(&wit)
        .arg("--js")
        .arg(&js)
        .arg("--output")
        .arg(&output)
        .arg("--stub-wasi")
        .assert()
        .success()
        .stdout(predicate::str::contains("Stubbing WASI imports"));

    assert!(output.exists(), "Output wasm file should exist");

    // Verify it runs with -S cli=n (no WASI implementation)
    let result = run_component_with_args(
        &output,
        env.dir.path(),
        "greet(\"World\")",
        &["-S", "cli=n"],
    );
    assert_eq!(result, "\"Hello, World!\"");

    let result = run_component_with_args(&output, env.dir.path(), "add(2, 3)", &["-S", "cli=n"]);
    assert_eq!(result, "5");
}
