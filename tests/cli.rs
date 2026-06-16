//! CLI integration tests for componentize-qjs
mod common;

use std::fs;

use predicates::prelude::*;
use tempfile::TempDir;
use wasmtime::Store;
use wasmtime::component::{Component, Linker, ResourceTable, Val};
use wasmtime_wasi::WasiCtxBuilder;

use common::{ComponentInstance, WasiCtxState, componentize_qjs, engine, run_cli_build};

#[test]
fn test_cli_help() {
    componentize_qjs()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: componentize-qjs"))
        .stdout(predicate::str::contains("--opt-size"))
        .stdout(predicate::str::contains("--sync"))
        .stdout(predicate::str::contains("--module-root <PATH>"))
        .stdout(predicate::str::contains("--runtime <PATH>"));
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

    let js_path = dir.path().join("test.js");
    fs::write(&js_path, "export {};").unwrap();
    let runtime_path = dir.path().join("runtime.wasm");
    fs::write(&runtime_path, componentize_qjs::default_runtime_wasm()).unwrap();

    componentize_qjs()
        .arg("--wit")
        .arg(&wit_path)
        .arg("--js")
        .arg(&js_path)
        .arg("--opt-size")
        .arg("--runtime")
        .arg(&runtime_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
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
fn test_cli_defers_host_imports_until_runtime() {
    let (output, _dir) = run_cli_build(
        r#"
            package local:test;

            interface math {
                add: func(a: s32, b: s32) -> s32;
                multiply: func(a: s32, b: s32) -> s32;
            }

            world imports {
                import math;
                export double-add: func(a: s32, b: s32) -> s32;
            }
        "#,
        r#"
            import math from "local:test/math";

            export function doubleAdd(a, b) {
                const sum = math.add(a, b);
                return math.multiply(sum, 2);
            }
        "#,
        &[],
    );

    let engine = engine();
    let component = Component::new(engine, fs::read(&output).unwrap()).unwrap();
    let mut store = Store::new(
        engine,
        WasiCtxState {
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
        },
    );

    let mut linker = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).unwrap();
    let mut math = linker.instance("local:test/math").unwrap();
    math.func_wrap("add", |_, (a, b): (i32, i32)| Ok((a + b,)))
        .unwrap();
    math.func_wrap("multiply", |_, (a, b): (i32, i32)| Ok((a * b,)))
        .unwrap();

    let instance = linker.instantiate(&mut store, &component).unwrap();
    let func = instance.get_func(&mut store, "double-add").unwrap();
    let mut results = [Val::S32(0)];
    func.call(&mut store, &[Val::S32(4), Val::S32(5)], &mut results)
        .unwrap();

    assert_eq!(results[0], Val::S32(18));
}

#[test]
fn test_cli_resolves_relative_import() {
    let dir = TempDir::new().unwrap();
    let wit_path = dir.path().join("test.wit");
    let js_path = dir.path().join("main.js");
    let dep_path = dir.path().join("dep.js");
    let output = dir.path().join("output.wasm");

    fs::write(
        &wit_path,
        "package test:modules;\nworld modules { export answer: func() -> u32; }",
    )
    .unwrap();
    fs::write(
        &js_path,
        r#"import { value } from "./dep.js"; export function answer() { return value + 1; }"#,
    )
    .unwrap();
    fs::write(&dep_path, "export const value = 41;").unwrap();

    componentize_qjs()
        .arg("--wit")
        .arg(&wit_path)
        .arg("--js")
        .arg(&js_path)
        .arg("--output")
        .arg(&output)
        .assert()
        .success();

    let wasm = fs::read(&output).unwrap();
    let mut inst =
        ComponentInstance::from_wasm(wasm, vec![], vec![]).expect("should instantiate component");

    assert_eq!(inst.call1("answer", &[]), Val::U32(42));
}

#[test]
fn test_cli_resolves_package_import_from_module_root() {
    let dir = TempDir::new().unwrap();
    let src_dir = dir.path().join("src");
    let pkg_dir = dir.path().join("node_modules").join("pkg");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&pkg_dir).unwrap();

    let wit_path = dir.path().join("test.wit");
    let js_path = src_dir.join("main.js");
    let output = dir.path().join("output.wasm");

    fs::write(
        &wit_path,
        "package test:modules;\nworld modules { export answer: func() -> u32; }",
    )
    .unwrap();
    fs::write(
        &js_path,
        r#"import { value } from "pkg"; export function answer() { return value + 1; }"#,
    )
    .unwrap();
    fs::write(pkg_dir.join("package.json"), r#"{"main":"index.js"}"#).unwrap();
    fs::write(pkg_dir.join("index.js"), "export const value = 41;").unwrap();

    componentize_qjs()
        .arg("--wit")
        .arg(&wit_path)
        .arg("--js")
        .arg(&js_path)
        .arg("--module-root")
        .arg(dir.path())
        .arg("--output")
        .arg(&output)
        .assert()
        .success();

    let wasm = fs::read(&output).unwrap();
    let mut inst =
        ComponentInstance::from_wasm(wasm, vec![], vec![]).expect("should instantiate component");

    assert_eq!(inst.call1("answer", &[]), Val::U32(42));
}

#[test]
fn test_cli_resolves_nested_imports_and_caches_modules() {
    let dir = TempDir::new().unwrap();
    let nested_dir = dir.path().join("nested");
    let index_dir = dir.path().join("dir");
    fs::create_dir_all(&nested_dir).unwrap();
    fs::create_dir_all(&index_dir).unwrap();

    let wit_path = dir.path().join("test.wit");
    let js_path = dir.path().join("main.js");
    let output = dir.path().join("output.wasm");

    fs::write(
        &wit_path,
        "package test:modules;\nworld modules { export answer: func() -> u32; }",
    )
    .unwrap();
    fs::write(
        &js_path,
        r#"
            import { nested } from "./nested/entry";
            import { fromIndex } from "./dir";
            import { count as countA } from "./a.js";
            import { count as countB } from "./b.js";

            export function answer() {
                return nested + fromIndex + countA + countB + globalThis.__counter;
            }
        "#,
    )
    .unwrap();
    fs::write(
        nested_dir.join("entry.js"),
        r#"import { base } from "./base"; export const nested = base * 2;"#,
    )
    .unwrap();
    fs::write(nested_dir.join("base.js"), "export const base = 10;").unwrap();
    fs::write(index_dir.join("index.js"), "export const fromIndex = 5;").unwrap();
    fs::write(
        dir.path().join("counter.js"),
        r#"
            globalThis.__counter = (globalThis.__counter ?? 0) + 1;
            export const count = globalThis.__counter;
        "#,
    )
    .unwrap();
    fs::write(
        dir.path().join("a.js"),
        r#"import { count } from "./counter.js"; export { count };"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("b.js"),
        r#"import { count } from "./counter.js"; export { count };"#,
    )
    .unwrap();

    componentize_qjs()
        .arg("--wit")
        .arg(&wit_path)
        .arg("--js")
        .arg(&js_path)
        .arg("--output")
        .arg(&output)
        .assert()
        .success();

    let wasm = fs::read(&output).unwrap();
    let mut inst =
        ComponentInstance::from_wasm(wasm, vec![], vec![]).expect("should instantiate component");

    assert_eq!(inst.call1("answer", &[]), Val::U32(28));
}

#[test]
fn test_cli_reports_missing_import() {
    let dir = TempDir::new().unwrap();
    let wit_path = dir.path().join("test.wit");
    let js_path = dir.path().join("main.js");
    let output = dir.path().join("output.wasm");

    fs::write(
        &wit_path,
        "package test:modules;\nworld modules { export answer: func() -> u32; }",
    )
    .unwrap();
    fs::write(
        &js_path,
        r#"import { value } from "./missing.js"; export function answer() { return value; }"#,
    )
    .unwrap();

    componentize_qjs()
        .arg("--wit")
        .arg(&wit_path)
        .arg("--js")
        .arg(&js_path)
        .arg("--output")
        .arg(&output)
        .assert()
        .failure()
        .stderr(predicate::str::contains("filesystem module not found"));
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
fn test_cli_opt_size_runtime() {
    let (output, _dir) = run_cli_build(
        "package test:runtime;\nworld runtime { export add: func(a: u32, b: u32) -> u32; }",
        "export function add(a, b) { return a + b; }",
        &["--opt-size"],
    );

    let wasm = fs::read(&output).unwrap();
    let mut inst = ComponentInstance::from_wasm(wasm, vec![], vec![])
        .expect("should instantiate opt-size runtime component");

    assert_eq!(inst.call1("add", &[Val::U32(3), Val::U32(4)]), Val::U32(7));
}

#[test]
fn test_cli_sync_runtime() {
    let (output, _dir) = run_cli_build(
        "package test:runtime;\nworld runtime { export add: func(a: u32, b: u32) -> u32; }",
        "export function add(a, b) { return a + b; }",
        &["--sync"],
    );

    let wasm = fs::read(&output).unwrap();
    let mut inst = ComponentInstance::from_wasm(wasm, vec![], vec![])
        .expect("should instantiate non-async runtime component");

    assert_eq!(inst.call1("add", &[Val::U32(3), Val::U32(4)]), Val::U32(7));
}

#[test]
fn test_cli_sync_opt_size_runtime() {
    let (output, _dir) = run_cli_build(
        "package test:runtime;\nworld runtime { export add: func(a: u32, b: u32) -> u32; }",
        "export function add(a, b) { return a + b; }",
        &["--sync", "--opt-size"],
    );

    let wasm = fs::read(&output).unwrap();
    let mut inst = ComponentInstance::from_wasm(wasm, vec![], vec![])
        .expect("should instantiate non-async opt-size runtime component");

    assert_eq!(inst.call1("add", &[Val::U32(3), Val::U32(4)]), Val::U32(7));
}

#[test]
fn test_cli_custom_runtime_file() {
    let dir = TempDir::new().unwrap();
    let wit_path = dir.path().join("test.wit");
    let js_path = dir.path().join("test.js");
    let output = dir.path().join("output.wasm");
    let runtime_path = dir.path().join("runtime.wasm");

    fs::write(
        &wit_path,
        "package test:runtime;\nworld runtime { export add: func(a: u32, b: u32) -> u32; }",
    )
    .unwrap();
    fs::write(&js_path, "export function add(a, b) { return a + b; }").unwrap();
    fs::write(&runtime_path, componentize_qjs::default_runtime_wasm()).unwrap();

    componentize_qjs()
        .arg("--wit")
        .arg(&wit_path)
        .arg("--js")
        .arg(&js_path)
        .arg("--output")
        .arg(&output)
        .arg("--runtime")
        .arg(&runtime_path)
        .assert()
        .success();

    let wasm = fs::read(&output).unwrap();
    let mut inst = ComponentInstance::from_wasm(wasm, vec![], vec![])
        .expect("should instantiate custom runtime component");

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
