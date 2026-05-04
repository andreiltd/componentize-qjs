//! CLI integration tests for componentize-qjs
mod common;

use std::fs;

use predicates::prelude::*;
use tempfile::TempDir;
use wasmtime::component::Val;

use common::{componentize_qjs, run_cli_build, ComponentInstance};

#[test]
fn test_cli_help() {
    componentize_qjs()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: componentize-qjs"));
}

#[test]
fn test_cli_errors() {
    componentize_qjs()
        .arg("--wit")
        .arg("nonexistent.wit")
        .arg("--js")
        .arg("test.js")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));

    let dir = TempDir::new().unwrap();
    let wit_path = dir.path().join("test.wit");
    fs::write(&wit_path, "package test:test; world test {}").unwrap();

    componentize_qjs()
        .arg("--wit")
        .arg(&wit_path)
        .arg("--js")
        .arg("nonexistent.js")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn test_cli_output() {
    let (output, _dir) = run_cli_build(
        "package test:hello;\nworld hello { export add: func(a: u32, b: u32) -> u32; }",
        "export function add(a, b) { return a + b; }",
        &[],
    );

    let wasm = fs::read(&output).unwrap();
    let mut inst =
        ComponentInstance::from_wasm(wasm, vec![], vec![]).expect("should instantiate component");

    assert_eq!(inst.call1("add", &[Val::U32(3), Val::U32(4)]), Val::U32(7));
}

#[test]
fn test_cli_stub_wasi() {
    let (output, _dir) = run_cli_build(
        "package test:hello;\nworld hello { export add: func(a: u32, b: u32) -> u32; }",
        "export function add(a, b) { return a + b; }",
        &["--stub-wasi"],
    );

    let wasm = fs::read(&output).unwrap();
    let mut inst = ComponentInstance::from_wasm(wasm, vec![], vec![])
        .expect("should instantiate stubbed component");

    assert_eq!(inst.call1("add", &[Val::U32(3), Val::U32(4)]), Val::U32(7));
}

#[test]
fn test_cli_minify() {
    let wit = r#"
        package test:minify;
        world minify-test {
            export add: func(a: u32, b: u32) -> u32;
            export greet: func(name: string) -> string;
        }
    "#;
    let js = r#"
        // This comment and whitespace should be stripped by minification
        // but the logic should remain identical

        /**
         * Foo bar baz.
         */
        export function add(a, b) {
            const result = a + b;
            return result;
        }

        /**
         * Foo bar baz.
         */
        export function greet(name) {
            const greeting = "Hello, " + name + "!";
            return greeting;
        }
    "#;

    let (output, _dir) = run_cli_build(wit, js, &["--minify"]);

    let wasm = fs::read(&output).unwrap();
    let mut inst = ComponentInstance::from_wasm(wasm, vec![], vec![])
        .expect("should instantiate minified component");

    assert_eq!(inst.call1("add", &[Val::U32(3), Val::U32(4)]), Val::U32(7));
    assert_eq!(
        inst.call1("greet", &[Val::String("World".into())]),
        Val::String("Hello, World!".into()),
    );
}
