//! Integration tests for componentize-qjs
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use predicates::prelude::*;
use tempfile::TempDir;
use wasmtime::component::{Component, Instance, Linker, ResourceTable, Val};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use componentize_qjs::ComponentizeOpts;

struct WasiCtxState {
    wasi: WasiCtx,
    table: ResourceTable,
}

impl WasiView for WasiCtxState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.wasm_component_model(true);
        Engine::new(&config).expect("Failed to create engine")
    })
}

struct Expectation {
    func_name: String,
    params: Vec<Val>,
    expected: Val,
}

/// Builder for constructing and running component tests.
struct TestCase {
    wit: Option<String>,
    wit_dir: Option<PathBuf>,
    world_name: Option<String>,
    script: Option<String>,
    stub_wasi: bool,
    env_vars: Vec<(String, String)>,
    expectations: Vec<Expectation>,
}

impl TestCase {
    fn new() -> Self {
        Self {
            wit: None,
            wit_dir: None,
            world_name: None,
            script: None,
            stub_wasi: false,
            env_vars: Vec::new(),
            expectations: Vec::new(),
        }
    }

    /// Set inline WIT source (written to a temp file).
    fn wit(mut self, wit: &str) -> Self {
        self.wit = Some(wit.to_string());
        self
    }

    /// Set path to a WIT directory (with deps/).
    fn wit_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.wit_dir = Some(path.into());
        self
    }

    /// Select a specific world from the WIT package.
    fn world(mut self, name: &str) -> Self {
        self.world_name = Some(name.to_string());
        self
    }

    fn script(mut self, js: &str) -> Self {
        self.script = Some(js.to_string());
        self
    }

    fn stub_wasi(mut self) -> Self {
        self.stub_wasi = true;
        self
    }

    /// Add an environment variable visible to the WASI context.
    fn env(mut self, key: &str, value: &str) -> Self {
        self.env_vars.push((key.to_string(), value.to_string()));
        self
    }

    /// Register an expected function call: name, params, and expected return value.
    fn expect_call(mut self, name: &str, params: Vec<Val>, expected: Val) -> Self {
        self.expectations.push(Expectation {
            func_name: name.to_string(),
            params,
            expected,
        });
        self
    }

    /// Build the component and return a live instance ready for calls.
    fn build(self) -> anyhow::Result<ComponentInstance> {
        let dir = TempDir::new()?;

        let wit_path = if let Some(ref wit_dir) = self.wit_dir {
            wit_dir.clone()
        } else {
            let p = dir.path().join("test.wit");
            fs::write(&p, self.wit.as_deref().unwrap())?;
            p
        };

        let opts = ComponentizeOpts {
            wit_path: &wit_path,
            js_source: self.script.as_deref().unwrap(),
            world_name: self.world_name.as_deref(),
            stub_wasi: self.stub_wasi,
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let wasm = rt.block_on(componentize_qjs::componentize(&opts))?;

        let engine = engine();
        let component = Component::new(engine, &wasm)?;

        let mut wasi_builder = WasiCtxBuilder::new();
        if !self.env_vars.is_empty() {
            wasi_builder.inherit_env();
            for (k, v) in &self.env_vars {
                wasi_builder.env(k, v);
            }
        }
        let wasi = wasi_builder.build();
        let table = ResourceTable::new();
        let mut store = Store::new(engine, WasiCtxState { wasi, table });

        let mut linker = Linker::new(engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;

        let instance = linker.instantiate(&mut store, &component)?;

        Ok(ComponentInstance {
            store,
            inner: instance,
            expectations: self.expectations,
        })
    }
}

struct ComponentInstance {
    store: Store<WasiCtxState>,
    inner: Instance,
    expectations: Vec<Expectation>,
}

impl ComponentInstance {
    /// Call an exported function with the given params and return results.
    fn call(&mut self, name: &str, params: &[Val], result_count: usize) -> Vec<Val> {
        let func = self
            .inner
            .get_func(&mut self.store, name)
            .unwrap_or_else(|| panic!("export `{name}` not found"));

        let mut results = vec![Val::Bool(false); result_count];
        func.call(&mut self.store, params, &mut results)
            .unwrap_or_else(|e| panic!("calling `{name}` failed: {e}"));

        results
    }

    /// Call an exported function expecting a single return value.
    fn call1(&mut self, name: &str, params: &[Val]) -> Val {
        self.call(name, params, 1).into_iter().next().unwrap()
    }

    /// Run all registered expectations, asserting each call matches.
    fn run(&mut self) {
        let expectations = std::mem::take(&mut self.expectations);

        for exp in expectations {
            let result = self.call1(&exp.func_name, &exp.params);
            assert_eq!(
                result, exp.expected,
                "calling `{}`: expected {:?}, got {:?}",
                exp.func_name, exp.expected, result
            );
        }
    }
}

fn componentize_qjs() -> assert_cmd::Command {
    assert_cmd::cargo::cargo_bin_cmd!()
}

#[test]
fn test_cli_output() {
    let dir = TempDir::new().unwrap();

    let wit_path = dir.path().join("hello.wit");
    fs::write(
        &wit_path,
        "package test:hello;\nworld hello { export add: func(a: u32, b: u32) -> u32; }",
    )
    .unwrap();

    let js_path = dir.path().join("hello.js");
    fs::write(&js_path, "function add(a, b) { return a + b; }").unwrap();

    let output = dir.path().join("hello.wasm");

    componentize_qjs()
        .arg("--wit")
        .arg(&wit_path)
        .arg("--js")
        .arg(&js_path)
        .arg("--output")
        .arg(&output)
        .assert()
        .success()
        .stdout(predicate::str::contains("Component written to"));

    assert!(output.exists(), "Output wasm file should exist");
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
fn test_cli_stub_wasi() {
    let dir = TempDir::new().unwrap();

    let wit_path = dir.path().join("hello.wit");
    fs::write(
        &wit_path,
        "package test:hello;\nworld hello { export add: func(a: u32, b: u32) -> u32; }",
    )
    .unwrap();

    let js_path = dir.path().join("hello.js");
    fs::write(&js_path, "function add(a, b) { return a + b; }").unwrap();

    let output = dir.path().join("hello-stubbed.wasm");

    componentize_qjs()
        .arg("--wit")
        .arg(&wit_path)
        .arg("--js")
        .arg(&js_path)
        .arg("--output")
        .arg(&output)
        .arg("--stub-wasi")
        .assert()
        .success()
        .stdout(predicate::str::contains("Stubbing WASI imports"));
}

#[test]
fn test_hello_world() {
    TestCase::new()
        .wit(
            r#"
            package test:hello;
            world hello {
                export greet: func() -> string;
                export add: func(a: u32, b: u32) -> u32;
            }
        "#,
        )
        .script(
            r#"
            function greet() { return "Hello, World!"; }
            function add(a, b) { return a + b; }
        "#,
        )
        .expect_call("greet", vec![], Val::String("Hello, World!".into()))
        .expect_call("add", vec![Val::U32(2), Val::U32(3)], Val::U32(5))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_numeric_types() {
    TestCase::new()
        .wit(
            r#"
            package test:types;
            world types {
                export add-u32: func(a: u32, b: u32) -> u32;
                export add-s32: func(a: s32, b: s32) -> s32;
                export add-f64: func(a: f64, b: f64) -> f64;
                export negate: func(b: bool) -> bool;
            }
        "#,
        )
        .script(
            r#"
            function addU32(a, b) { return a + b; }
            function addS32(a, b) { return a + b; }
            function addF64(a, b) { return a + b; }
            function negate(b) { return !b; }
        "#,
        )
        .expect_call("add-u32", vec![Val::U32(100), Val::U32(200)], Val::U32(300))
        .expect_call("add-s32", vec![Val::S32(-10), Val::S32(5)], Val::S32(-5))
        .expect_call(
            "add-f64",
            vec![Val::Float64(1.5), Val::Float64(2.5)],
            Val::Float64(4.0),
        )
        .expect_call("negate", vec![Val::Bool(true)], Val::Bool(false))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_record_type() {
    let point = |x: f64, y: f64| {
        Val::Record(vec![
            ("x".into(), Val::Float64(x)),
            ("y".into(), Val::Float64(y)),
        ])
    };

    TestCase::new()
        .wit(
            r#"
            package test:records;
            world record-test {
                record point { x: f64, y: f64 }
                export add-points: func(a: point, b: point) -> point;
            }
        "#,
        )
        .script("function addPoints(a, b) { return { x: a.x + b.x, y: a.y + b.y }; }")
        .expect_call(
            "add-points",
            vec![point(1.0, 2.0), point(3.0, 4.0)],
            point(4.0, 6.0),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_list_type() {
    TestCase::new()
        .wit(
            r#"
            package test:lists;
            world list-test {
                export sum-list: func(nums: list<u32>) -> u32;
            }
        "#,
        )
        .script("function sumList(nums) { return nums.reduce((a, b) => a + b, 0); }")
        .expect_call(
            "sum-list",
            vec![Val::List(vec![
                Val::U32(1),
                Val::U32(2),
                Val::U32(3),
                Val::U32(4),
                Val::U32(5),
            ])],
            Val::U32(15),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_option_type() {
    TestCase::new()
        .wit(
            r#"
            package test:options;
            world option-test {
                export maybe-double: func(n: option<u32>) -> option<u32>;
            }
        "#,
        )
        .script(
            r#"
            function maybeDouble(n) {
                if (n === null || n === undefined) { return null; }
                return n * 2;
            }
        "#,
        )
        .expect_call(
            "maybe-double",
            vec![Val::Option(Some(Box::new(Val::U32(5))))],
            Val::Option(Some(Box::new(Val::U32(10)))),
        )
        .expect_call("maybe-double", vec![Val::Option(None)], Val::Option(None))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_result_type() {
    TestCase::new()
        .wit(
            r#"
            package test:results;
            world result-test {
                export safe-div: func(a: u32, b: u32) -> result<u32, string>;
            }
        "#,
        )
        .script(
            r#"
            function safeDiv(a, b) {
                if (b === 0) { return { tag: "err", val: "division by zero" }; }
                return { tag: "ok", val: Math.floor(a / b) };
            }
        "#,
        )
        .expect_call(
            "safe-div",
            vec![Val::U32(10), Val::U32(2)],
            Val::Result(Ok(Some(Box::new(Val::U32(5))))),
        )
        .expect_call(
            "safe-div",
            vec![Val::U32(10), Val::U32(0)],
            Val::Result(Err(Some(Box::new(Val::String("division by zero".into()))))),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_stub_wasi() {
    TestCase::new()
        .wit(
            r#"
            package test:hello;
            world hello {
                export greet: func(name: string) -> string;
                export add: func(a: u32, b: u32) -> u32;
            }
        "#,
        )
        .script(
            r#"
            function greet(name) { return "Hello, " + name + "!"; }
            function add(a, b) { return a + b; }
        "#,
        )
        .stub_wasi()
        .expect_call(
            "greet",
            vec![Val::String("World".into())],
            Val::String("Hello, World!".into()),
        )
        .expect_call("add", vec![Val::U32(2), Val::U32(3)], Val::U32(5))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_all_integer_types() {
    TestCase::new()
        .wit(
            r#"
            package test:integers;
            world integers {
                export add-u8: func(a: u8, b: u8) -> u8;
                export add-s8: func(a: s8, b: s8) -> s8;
                export add-u16: func(a: u16, b: u16) -> u16;
                export add-s16: func(a: s16, b: s16) -> s16;
                export add-u64: func(a: u64, b: u64) -> u64;
                export add-s64: func(a: s64, b: s64) -> s64;
            }
        "#,
        )
        .script(
            r#"
            function addU8(a, b) { return a + b; }
            function addS8(a, b) { return a + b; }
            function addU16(a, b) { return a + b; }
            function addS16(a, b) { return a + b; }
            function addU64(a, b) { return a + b; }
            function addS64(a, b) { return a + b; }
        "#,
        )
        .expect_call("add-u8", vec![Val::U8(200), Val::U8(55)], Val::U8(255))
        .expect_call("add-s8", vec![Val::S8(-100), Val::S8(50)], Val::S8(-50))
        .expect_call(
            "add-u16",
            vec![Val::U16(60000), Val::U16(5535)],
            Val::U16(65535),
        )
        .expect_call(
            "add-s16",
            vec![Val::S16(-30000), Val::S16(10000)],
            Val::S16(-20000),
        )
        .expect_call(
            "add-u64",
            vec![Val::U64(1_000_000_000), Val::U64(2_000_000_000)],
            Val::U64(3_000_000_000),
        )
        .expect_call(
            "add-s64",
            vec![Val::S64(-1_000_000_000), Val::S64(500_000_000)],
            Val::S64(-500_000_000),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_float_types() {
    TestCase::new()
        .wit(
            r#"
            package test:floats;
            world floats {
                export add-f32: func(a: f32, b: f32) -> f32;
                export add-f64: func(a: f64, b: f64) -> f64;
            }
        "#,
        )
        .script("function addF32(a, b) { return a + b; }\nfunction addF64(a, b) { return a + b; }")
        .expect_call(
            "add-f32",
            vec![Val::Float32(1.5), Val::Float32(2.5)],
            Val::Float32(4.0),
        )
        .expect_call(
            "add-f64",
            vec![Val::Float64(1.5), Val::Float64(2.5)],
            Val::Float64(4.0),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_string_operations() {
    TestCase::new()
        .wit(
            r#"
            package test:strings;
            world strings {
                export take-string: func(s: string) -> u32;
                export return-string: func() -> string;
                export concat-strings: func(a: string, b: string) -> string;
            }
        "#,
        )
        .script(
            r#"
            function takeString(s) { return s.length; }
            function returnString() { return "hello from js"; }
            function concatStrings(a, b) { return a + b; }
        "#,
        )
        .expect_call(
            "take-string",
            vec![Val::String("hello".into())],
            Val::U32(5),
        )
        .expect_call("return-string", vec![], Val::String("hello from js".into()))
        .expect_call(
            "concat-strings",
            vec![Val::String("foo".into()), Val::String("bar".into())],
            Val::String("foobar".into()),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_char_type() {
    TestCase::new()
        .wit(
            r#"
            package test:chars;
            world chars {
                export take-char: func(c: char) -> u32;
                export return-char: func() -> char;
            }
        "#,
        )
        .script(
            r#"
            function takeChar(c) { return c.codePointAt(0); }
            function returnChar() { return "A"; }
        "#,
        )
        .expect_call("take-char", vec![Val::Char('A')], Val::U32(65))
        .expect_call("return-char", vec![], Val::Char('A'))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_enum_type() {
    // Enums are represented as numeric discriminants (0, 1, 2, ...) in JS
    TestCase::new()
        .wit(
            r#"
            package test:enums;
            world enums {
                enum color { red, green, blue }
                export identify-color: func(c: color) -> string;
                export favorite-color: func() -> color;
            }
        "#,
        )
        .script(
            r#"
            function identifyColor(c) {
                if (c === 0) return "is red";
                if (c === 1) return "is green";
                if (c === 2) return "is blue";
                return "unknown";
            }
            function favoriteColor() { return 1; }
        "#,
        )
        .expect_call(
            "identify-color",
            vec![Val::Enum("red".into())],
            Val::String("is red".into()),
        )
        .expect_call(
            "identify-color",
            vec![Val::Enum("blue".into())],
            Val::String("is blue".into()),
        )
        .expect_call("favorite-color", vec![], Val::Enum("green".into()))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_variant_type() {
    // Variants use numeric tags (0, 1, ...) and {tag, val} objects in JS
    TestCase::new()
        .wit(
            r#"
            package test:variants;
            world variants {
                variant shape { circle(f64), none }
                export describe-shape: func(s: shape) -> string;
                export make-circle: func(r: f64) -> shape;
            }
        "#,
        )
        .script(
            r#"
            function describeShape(s) {
                if (s.tag === 0) return "circle with radius " + s.val;
                if (s.tag === 1) return "no shape";
                return "unknown";
            }
            function makeCircle(r) { return { tag: 0, val: r }; }
        "#,
        )
        .expect_call(
            "describe-shape",
            vec![Val::Variant(
                "circle".into(),
                Some(Box::new(Val::Float64(3.5))),
            )],
            Val::String("circle with radius 3.5".into()),
        )
        .expect_call(
            "describe-shape",
            vec![Val::Variant("none".into(), None)],
            Val::String("no shape".into()),
        )
        .expect_call(
            "make-circle",
            vec![Val::Float64(2.0)],
            Val::Variant("circle".into(), Some(Box::new(Val::Float64(2.0)))),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_flag_type() {
    // Flags are represented as bitmask numbers in JS
    TestCase::new()
        .wit(
            r#"
            package test:flagtest;
            world flag-test {
                flags permissions { read, write, execute }
                export check-read: func(p: permissions) -> bool;
                export read-write: func() -> permissions;
            }
        "#,
        )
        .script(
            "function checkRead(p) { return (p & 1) !== 0; }\nfunction readWrite() { return 3; }",
        )
        .expect_call(
            "check-read",
            vec![Val::Flags(vec!["read".into(), "write".into()])],
            Val::Bool(true),
        )
        .expect_call(
            "check-read",
            vec![Val::Flags(vec!["execute".into()])],
            Val::Bool(false),
        )
        .expect_call(
            "read-write",
            vec![],
            Val::Flags(vec!["read".into(), "write".into()]),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_tuple_return() {
    TestCase::new()
        .wit(
            r#"
            package test:tuples;
            world tuples {
                export swap: func(a: u32, b: u32) -> tuple<u32, u32>;
            }
        "#,
        )
        .script("function swap(a, b) { return [b, a]; }")
        .expect_call(
            "swap",
            vec![Val::U32(1), Val::U32(2)],
            Val::Tuple(vec![Val::U32(2), Val::U32(1)]),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_many_arguments() {
    let params: Vec<Val> = (1..=10).map(Val::U32).collect();

    TestCase::new()
        .wit(r#"
            package test:manyargs;
            world many-args {
                export sum-ten: func(a1: u32, a2: u32, a3: u32, a4: u32, a5: u32, a6: u32, a7: u32, a8: u32, a9: u32, a10: u32) -> u32;
            }
        "#)
        .script(r#"
            function sumTen(a1, a2, a3, a4, a5, a6, a7, a8, a9, a10) {
                return a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8 + a9 + a10;
            }
        "#)
        .expect_call("sum-ten", params, Val::U32(55))
        .build().unwrap()
        .run();
}

#[test]
fn test_no_arg_functions() {
    TestCase::new()
        .wit(
            r#"
            package test:noargs;
            world noargs {
                export get-answer: func() -> u32;
                export get-message: func() -> string;
                export get-flag: func() -> bool;
            }
        "#,
        )
        .script(
            r#"
            function getAnswer() { return 42; }
            function getMessage() { return "hello"; }
            function getFlag() { return true; }
        "#,
        )
        .expect_call("get-answer", vec![], Val::U32(42))
        .expect_call("get-message", vec![], Val::String("hello".into()))
        .expect_call("get-flag", vec![], Val::Bool(true))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_nested_lists() {
    let nested = Val::List(vec![
        Val::List(vec![Val::U32(1), Val::U32(2)]),
        Val::List(vec![Val::U32(3), Val::U32(4)]),
        Val::List(vec![Val::U32(5)]),
    ]);
    let expected = Val::List(vec![
        Val::U32(1),
        Val::U32(2),
        Val::U32(3),
        Val::U32(4),
        Val::U32(5),
    ]);

    TestCase::new()
        .wit(
            r#"
            package test:nested;
            world nested-lists {
                export flatten: func(nested: list<list<u32>>) -> list<u32>;
            }
        "#,
        )
        .script(
            "function flatten(nested) { return nested.reduce((acc, arr) => acc.concat(arr), []); }",
        )
        .expect_call("flatten", vec![nested], expected)
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_complex_record() {
    let alice = Val::Record(vec![
        ("name".into(), Val::String("Alice".into())),
        ("age".into(), Val::U32(30)),
        ("active".into(), Val::Bool(true)),
    ]);
    let bob = Val::Record(vec![
        ("name".into(), Val::String("Bob".into())),
        ("age".into(), Val::U32(25)),
        ("active".into(), Val::Bool(true)),
    ]);

    TestCase::new()
        .wit(r#"
            package test:complex;
            world complex-record {
                record person { name: string, age: u32, active: bool }
                export greet-person: func(p: person) -> string;
                export make-person: func(name: string, age: u32) -> person;
            }
        "#)
        .script(r#"
            function greetPerson(p) { return "Hello " + p.name + ", age " + p.age + ", active: " + p.active; }
            function makePerson(name, age) { return { name: name, age: age, active: true }; }
        "#)
        .expect_call("greet-person", vec![alice], Val::String("Hello Alice, age 30, active: true".into()))
        .expect_call("make-person", vec![Val::String("Bob".into()), Val::U32(25)], bob)
        .build().unwrap()
        .run();
}

#[test]
fn test_list_of_strings() {
    TestCase::new()
        .wit(
            r#"
            package test:stringlists;
            world string-lists {
                export join-strings: func(parts: list<string>, sep: string) -> string;
                export count-strings: func(parts: list<string>) -> u32;
            }
        "#,
        )
        .script(
            r#"
            function joinStrings(parts, sep) { return parts.join(sep); }
            function countStrings(parts) { return parts.length; }
        "#,
        )
        .expect_call(
            "join-strings",
            vec![
                Val::List(vec![
                    Val::String("a".into()),
                    Val::String("b".into()),
                    Val::String("c".into()),
                ]),
                Val::String("-".into()),
            ],
            Val::String("a-b-c".into()),
        )
        .expect_call(
            "count-strings",
            vec![Val::List(vec![
                Val::String("one".into()),
                Val::String("two".into()),
                Val::String("three".into()),
            ])],
            Val::U32(3),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_empty_world() {
    TestCase::new()
        .wit(
            r#"
            package test:empty;
            world empty {}
        "#,
        )
        .script("// empty module\n")
        .build()
        .unwrap();
}

#[test]
fn test_naming_conventions() {
    // WIT kebab-case becomes camelCase in JS
    let rec = Val::Record(vec![
        ("first-name".into(), Val::String("John".into())),
        ("last-name".into(), Val::String("Doe".into())),
    ]);

    TestCase::new()
        .wit(
            r#"
            package test:conventions;
            world conventions {
                record my-record { first-name: string, last-name: string }
                export get-full-name: func(r: my-record) -> string;
            }
        "#,
        )
        .script(r#"function getFullName(r) { return r.firstName + " " + r.lastName; }"#)
        .expect_call("get-full-name", vec![rec], Val::String("John Doe".into()))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_repeated_calls() {
    let mut inst = TestCase::new()
        .wit(
            r#"
            package test:repeated;
            world repeated {
                export hello: func() -> string;
            }
        "#,
        )
        .script(r#"function hello() { return "hello"; }"#)
        .build()
        .unwrap();

    for _ in 0..5 {
        assert_eq!(inst.call1("hello", &[]), Val::String("hello".into()));
    }
}

fn wasi_wit_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/wit")
}

#[test]
fn test_wasi_random() {
    let mut inst = TestCase::new()
        .wit_dir(wasi_wit_dir())
        .world("wasi-random")
        .script(
            r#"
            const random = globalThis["wasi:random/random@0.2.6"];
            function getRandomU64() { return random.getRandomU64(); }
            function getRandomBytes(len) { return random.getRandomBytes(len); }
        "#,
        )
        .build()
        .expect("should build wasi-random component");

    let val = inst.call1("get-random-u64", &[]);
    assert!(matches!(val, Val::U64(_)), "Expected u64, got: {:?}", val);

    let val2 = inst.call1("get-random-u64", &[]);
    assert_ne!(val, val2, "Two random u64 calls returned the same value");

    let bytes = inst.call1("get-random-bytes", &[Val::U32(16)]);
    match &bytes {
        Val::List(items) => assert_eq!(items.len(), 16, "Expected 16 random bytes"),
        other => panic!("Expected list, got: {:?}", other),
    }
}

#[test]
fn test_wasi_clocks() {
    let mut inst = TestCase::new()
        .wit_dir(wasi_wit_dir())
        .world("wasi-clocks")
        .script(
            r#"
            const clock = globalThis["wasi:clocks/monotonic-clock@0.2.6"];
            function getTimeNs() { return clock.now(); }
            function elapsedNs() {
                const start = clock.now();
                // Do some trivial work to burn a tiny bit of time
                let x = 0;
                for (let i = 0; i < 1000; i++) { x += i; }
                return clock.now() - start;
            }
        "#,
        )
        .build()
        .expect("should build wasi-clocks component");

    // Monotonic clock should return a positive timestamp
    let time = inst.call1("get-time-ns", &[]);
    match time {
        Val::U64(ns) => assert!(ns > 0, "Monotonic clock returned 0"),
        other => panic!("Expected u64, got: {:?}", other),
    }

    // Second call should be >= first
    let time2 = inst.call1("get-time-ns", &[]);
    match (&time, &time2) {
        (Val::U64(t1), Val::U64(t2)) => assert!(t2 >= t1, "Clock went backwards: {} -> {}", t1, t2),
        _ => unreachable!(),
    }

    // Elapsed should return a u64
    let elapsed = inst.call1("elapsed-ns", &[]);
    assert!(
        matches!(elapsed, Val::U64(_)),
        "Expected u64, got: {:?}",
        elapsed
    );
}

#[test]
fn test_wasi_environment() {
    let mut inst = TestCase::new()
        .wit_dir(wasi_wit_dir())
        .world("wasi-environment")
        .env("TEST_KEY", "test_value")
        .script(
            r#"
            const env = globalThis["wasi:cli/environment@0.2.6"];
            function getEnvVars() { return env.getEnvironment(); }
        "#,
        )
        .build()
        .expect("should build wasi-environment component");

    let vars = inst.call1("get-env-vars", &[]);
    match &vars {
        Val::List(items) => {
            let found = items.iter().any(|item| {
                matches!(item, Val::Tuple(fields) if
                    fields.len() == 2
                    && fields[0] == Val::String("TEST_KEY".into())
                    && fields[1] == Val::String("test_value".into())
                )
            });
            assert!(
                found,
                "TEST_KEY=test_value not found in env vars: {:?}",
                items
            );
        }
        other => panic!("Expected list, got: {:?}", other),
    }
}
