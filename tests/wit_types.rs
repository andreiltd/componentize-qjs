//! WIT type integration tests for componentize-qjs
mod common;

use wasmtime::component::Val;

use common::TestCase;

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

#[test]
fn test_deeply_nested_lists() {
    // 3 levels: list<list<list<u32>>>
    let input = Val::List(vec![
        Val::List(vec![
            Val::List(vec![Val::U32(1), Val::U32(2)]),
            Val::List(vec![Val::U32(3)]),
        ]),
        Val::List(vec![Val::List(vec![Val::U32(4), Val::U32(5), Val::U32(6)])]),
    ]);

    TestCase::new()
        .wit(
            r#"
            package test:deep-nesting;
            world deep-nesting {
                export deep-flatten: func(nested: list<list<list<u32>>>) -> list<u32>;
            }
        "#,
        )
        .script(
            r#"
            function deepFlatten(nested) {
                let result = [];
                for (const mid of nested) {
                    for (const inner of mid) {
                        for (const v of inner) {
                            result.push(v);
                        }
                    }
                }
                return result;
            }
        "#,
        )
        .expect_call(
            "deep-flatten",
            vec![input],
            Val::List(vec![
                Val::U32(1),
                Val::U32(2),
                Val::U32(3),
                Val::U32(4),
                Val::U32(5),
                Val::U32(6),
            ]),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_nested_option() {
    // option<option<u32>>
    TestCase::new()
        .wit(
            r#"
            package test:nested-option;
            world nested-option {
                export unwrap-nested: func(val: option<option<u32>>) -> u32;
            }
        "#,
        )
        .script(
            r#"
            function unwrapNested(val) {
                if (val === null || val === undefined) return 0;
                if (val === null || val === undefined) return 0;
                return val;
            }
        "#,
        )
        .stub_wasi()
        .expect_call(
            "unwrap-nested",
            vec![Val::Option(Some(Box::new(Val::Option(Some(Box::new(
                Val::U32(42),
            ))))))],
            Val::U32(42),
        )
        .expect_call("unwrap-nested", vec![Val::Option(None)], Val::U32(0))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_list_of_records() {
    let people = Val::List(vec![
        Val::Record(vec![
            ("name".into(), Val::String("Alice".into())),
            ("score".into(), Val::U32(90)),
        ]),
        Val::Record(vec![
            ("name".into(), Val::String("Bob".into())),
            ("score".into(), Val::U32(85)),
        ]),
    ]);

    TestCase::new()
        .wit(
            r#"
            package test:list-records;
            world list-records {
                record player { name: string, score: u32 }
                export total-score: func(players: list<player>) -> u32;
                export top-player: func(players: list<player>) -> string;
            }
        "#,
        )
        .script(
            r#"
            function totalScore(players) {
                return players.reduce((sum, p) => sum + p.score, 0);
            }
            function topPlayer(players) {
                let best = players[0];
                for (const p of players) {
                    if (p.score > best.score) best = p;
                }
                return best.name;
            }
        "#,
        )
        .expect_call("total-score", vec![people.clone()], Val::U32(175))
        .expect_call("top-player", vec![people], Val::String("Alice".into()))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_list_of_variants() {
    // list<variant> round-trip
    TestCase::new()
        .wit(
            r#"
            package test:list-variants;
            world list-variants {
                variant item { text(string), number(u32), empty }
                export count-texts: func(items: list<item>) -> u32;
            }
        "#,
        )
        .script(
            r#"
            function countTexts(items) {
                let count = 0;
                for (const item of items) {
                    if (item.tag === 0) count++;
                }
                return count;
            }
        "#,
        )
        .expect_call(
            "count-texts",
            vec![Val::List(vec![
                Val::Variant("text".into(), Some(Box::new(Val::String("hello".into())))),
                Val::Variant("number".into(), Some(Box::new(Val::U32(42)))),
                Val::Variant("text".into(), Some(Box::new(Val::String("world".into())))),
                Val::Variant("empty".into(), None),
            ])],
            Val::U32(2),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_record_with_nested_fields() {
    // Record containing option, list, and result fields
    TestCase::new()
        .wit(
            r#"
            package test:nested-record;
            world nested-record {
                record config {
                    name: string,
                    tags: list<string>,
                    max-retries: option<u32>,
                }
                export describe-config: func(c: config) -> string;
                export make-config: func(name: string) -> config;
            }
        "#,
        )
        .script(
            r#"
            function describeConfig(c) {
                let s = c.name + ": tags=" + c.tags.join(",");
                if (c.maxRetries !== null && c.maxRetries !== undefined) {
                    s += " retries=" + c.maxRetries;
                }
                return s;
            }
            function makeConfig(name) {
                return { name: name, tags: ["default"], maxRetries: 3 };
            }
        "#,
        )
        .expect_call(
            "describe-config",
            vec![Val::Record(vec![
                ("name".into(), Val::String("test".into())),
                (
                    "tags".into(),
                    Val::List(vec![Val::String("a".into()), Val::String("b".into())]),
                ),
                (
                    "max-retries".into(),
                    Val::Option(Some(Box::new(Val::U32(5)))),
                ),
            ])],
            Val::String("test: tags=a,b retries=5".into()),
        )
        .expect_call(
            "make-config",
            vec![Val::String("prod".into())],
            Val::Record(vec![
                ("name".into(), Val::String("prod".into())),
                (
                    "tags".into(),
                    Val::List(vec![Val::String("default".into())]),
                ),
                (
                    "max-retries".into(),
                    Val::Option(Some(Box::new(Val::U32(3)))),
                ),
            ]),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_option_of_result() {
    TestCase::new()
        .wit(
            r#"
            package test:option-result;
            world option-result {
                export process: func(val: option<result<u32, string>>) -> string;
            }
        "#,
        )
        .script(
            r#"
            function process(val) {
                if (val === null || val === undefined) return "none";
                if (val.tag === "ok") return "ok:" + val.val;
                return "err:" + val.val;
            }
        "#,
        )
        .stub_wasi()
        .expect_call(
            "process",
            vec![Val::Option(Some(Box::new(Val::Result(Ok(Some(
                Box::new(Val::U32(42)),
            ))))))],
            Val::String("ok:42".into()),
        )
        .expect_call(
            "process",
            vec![Val::Option(Some(Box::new(Val::Result(Err(Some(
                Box::new(Val::String("fail".into())),
            ))))))],
            Val::String("err:fail".into()),
        )
        .expect_call(
            "process",
            vec![Val::Option(None)],
            Val::String("none".into()),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_result_of_option() {
    TestCase::new()
        .wit(
            r#"
            package test:result-option;
            world result-option {
                export maybe-lookup: func(key: string) -> result<option<u32>, string>;
            }
        "#,
        )
        .script(
            r#"
            function maybeLookup(key) {
                if (key === "found") return { tag: "ok", val: 42 };
                if (key === "missing") return { tag: "ok", val: null };
                return { tag: "err", val: "invalid key" };
            }
        "#,
        )
        .stub_wasi()
        .expect_call(
            "maybe-lookup",
            vec![Val::String("found".into())],
            Val::Result(Ok(Some(Box::new(Val::Option(Some(Box::new(Val::U32(
                42,
            )))))))),
        )
        .expect_call(
            "maybe-lookup",
            vec![Val::String("missing".into())],
            Val::Result(Ok(Some(Box::new(Val::Option(None))))),
        )
        .expect_call(
            "maybe-lookup",
            vec![Val::String("error".into())],
            Val::Result(Err(Some(Box::new(Val::String("invalid key".into()))))),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_empty_list() {
    TestCase::new()
        .wit(
            r#"
            package test:empty-list;
            world empty-list {
                export count: func(items: list<u32>) -> u32;
                export make-empty: func() -> list<u32>;
            }
        "#,
        )
        .script(
            r#"
            function count(items) { return items.length; }
            function makeEmpty() { return []; }
        "#,
        )
        .stub_wasi()
        .expect_call("count", vec![Val::List(vec![])], Val::U32(0))
        .expect_call("make-empty", vec![], Val::List(vec![]))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_import_export_chain() {
    let mut inst = TestCase::new()
        .wit(
            r#"
            package test:chain;
            world chain {
                export process: func(val: u32) -> u32;
            }
        "#,
        )
        .script(
            r#"
            function process(val) {
                return val + 11;
            }
        "#,
        )
        .stub_wasi()
        .expect_call("process", vec![Val::U32(5)], Val::U32(16))
        .build()
        .unwrap();

    inst.run();
}

#[test]
fn test_multiple_return_results() {
    // Test result types with no error payload
    TestCase::new()
        .wit(
            r#"
            package test:result-void;
            world result-void {
                export try-op: func(succeed: bool) -> result<u32>;
            }
        "#,
        )
        .script(
            r#"
            function tryOp(succeed) {
                if (succeed) return { tag: "ok", val: 42 };
                return { tag: "err" };
            }
        "#,
        )
        .stub_wasi()
        .expect_call(
            "try-op",
            vec![Val::Bool(true)],
            Val::Result(Ok(Some(Box::new(Val::U32(42))))),
        )
        .expect_call("try-op", vec![Val::Bool(false)], Val::Result(Err(None)))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_variant_with_multiple_payload_types() {
    // Variant with different payload types including string and none
    TestCase::new()
        .wit(
            r#"
            package test:multi-variant;
            world multi-variant {
                variant value { integer(s32), text(string), flag(bool), nothing }
                export stringify: func(v: value) -> string;
                export make-text: func(s: string) -> value;
                export make-nothing: func() -> value;
            }
        "#,
        )
        .script(
            r#"
            function stringify(v) {
                if (v.tag === 0) return "int:" + v.val;
                if (v.tag === 1) return "text:" + v.val;
                if (v.tag === 2) return "flag:" + v.val;
                return "nothing";
            }
            function makeText(s) { return { tag: 1, val: s }; }
            function makeNothing() { return { tag: 3 }; }
        "#,
        )
        .stub_wasi()
        .expect_call(
            "stringify",
            vec![Val::Variant(
                "integer".into(),
                Some(Box::new(Val::S32(-42))),
            )],
            Val::String("int:-42".into()),
        )
        .expect_call(
            "stringify",
            vec![Val::Variant(
                "text".into(),
                Some(Box::new(Val::String("hello".into()))),
            )],
            Val::String("text:hello".into()),
        )
        .expect_call(
            "stringify",
            vec![Val::Variant("nothing".into(), None)],
            Val::String("nothing".into()),
        )
        .expect_call(
            "make-text",
            vec![Val::String("world".into())],
            Val::Variant("text".into(), Some(Box::new(Val::String("world".into())))),
        )
        .expect_call("make-nothing", vec![], Val::Variant("nothing".into(), None))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_tuple_of_mixed_types() {
    TestCase::new()
        .wit(
            r#"
            package test:mixed-tuple;
            world mixed-tuple {
                export first: func(t: tuple<string, u32, bool>) -> string;
                export make-tuple: func() -> tuple<string, u32, bool>;
            }
        "#,
        )
        .script(
            r#"
            function first(t) { return t[0]; }
            function makeTuple() { return ["hello", 42, true]; }
        "#,
        )
        .stub_wasi()
        .expect_call(
            "first",
            vec![Val::Tuple(vec![
                Val::String("test".into()),
                Val::U32(99),
                Val::Bool(false),
            ])],
            Val::String("test".into()),
        )
        .expect_call(
            "make-tuple",
            vec![],
            Val::Tuple(vec![
                Val::String("hello".into()),
                Val::U32(42),
                Val::Bool(true),
            ]),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_many_params_echo() {
    // Matching compjs's echo_many — 8 diverse params
    TestCase::new()
        .wit(
            r#"
            package test:many-echo;
            world many-echo {
                export echo-many: func(
                    a: bool, b: u8, c: s16, d: u32,
                    e: s64, f: f32, g: f64, h: string
                ) -> string;
            }
        "#,
        )
        .script(
            r#"
            function echoMany(a, b, c, d, e, f, g, h) {
                return [a, b, c, d, e, f.toFixed(1), g.toFixed(1), h].join(",");
            }
        "#,
        )
        .stub_wasi()
        .expect_call(
            "echo-many",
            vec![
                Val::Bool(true),
                Val::U8(255),
                Val::S16(-100),
                Val::U32(1000),
                Val::S64(-9999),
                Val::Float32(std::f32::consts::PI),
                Val::Float64(std::f64::consts::E),
                Val::String("end".into()),
            ],
            Val::String("true,255,-100,1000,-9999,3.1,2.7,end".into()),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_list_of_options() {
    TestCase::new()
        .wit(
            r#"
            package test:list-options;
            world list-options {
                export count-some: func(items: list<option<u32>>) -> u32;
            }
        "#,
        )
        .script(
            r#"
            function countSome(items) {
                let count = 0;
                for (const item of items) {
                    if (item !== null && item !== undefined) count++;
                }
                return count;
            }
        "#,
        )
        .stub_wasi()
        .expect_call(
            "count-some",
            vec![Val::List(vec![
                Val::Option(Some(Box::new(Val::U32(1)))),
                Val::Option(None),
                Val::Option(Some(Box::new(Val::U32(3)))),
                Val::Option(None),
                Val::Option(Some(Box::new(Val::U32(5)))),
            ])],
            Val::U32(3),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_list_of_tuples() {
    TestCase::new()
        .wit(
            r#"
            package test:list-tuples;
            world list-tuples {
                export sum-pairs: func(pairs: list<tuple<u32, u32>>) -> u32;
            }
        "#,
        )
        .script(
            r#"
            function sumPairs(pairs) {
                return pairs.reduce((sum, p) => sum + p[0] + p[1], 0);
            }
        "#,
        )
        .stub_wasi()
        .expect_call(
            "sum-pairs",
            vec![Val::List(vec![
                Val::Tuple(vec![Val::U32(1), Val::U32(2)]),
                Val::Tuple(vec![Val::U32(3), Val::U32(4)]),
                Val::Tuple(vec![Val::U32(5), Val::U32(6)]),
            ])],
            Val::U32(21),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_result_of_result() {
    // Nested result types
    TestCase::new()
        .wit(
            r#"
            package test:nested-result;
            world nested-result {
                export try-nested: func(level: u32) -> result<result<u32, string>, string>;
            }
        "#,
        )
        .script(
            r#"
            function tryNested(level) {
                if (level === 0) return { tag: "err", val: "outer error" };
                if (level === 1) return { tag: "ok", val: { tag: "err", val: "inner error" } };
                return { tag: "ok", val: { tag: "ok", val: level * 10 } };
            }
        "#,
        )
        .stub_wasi()
        .expect_call(
            "try-nested",
            vec![Val::U32(0)],
            Val::Result(Err(Some(Box::new(Val::String("outer error".into()))))),
        )
        .expect_call(
            "try-nested",
            vec![Val::U32(1)],
            Val::Result(Ok(Some(Box::new(Val::Result(Err(Some(Box::new(
                Val::String("inner error".into()),
            )))))))),
        )
        .expect_call(
            "try-nested",
            vec![Val::U32(2)],
            Val::Result(Ok(Some(Box::new(Val::Result(Ok(Some(Box::new(
                Val::U32(20),
            )))))))),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_signed_integer_boundaries() {
    TestCase::new()
        .wit(
            r#"
            package test:signed-bounds;
            world signed-bounds {
                export echo-s32: func(v: s32) -> s32;
                export echo-s64: func(v: s64) -> s64;
            }
        "#,
        )
        .script(
            r#"
            function echoS32(v) { return v; }
            function echoS64(v) { return v; }
        "#,
        )
        .stub_wasi()
        .expect_call("echo-s32", vec![Val::S32(-1)], Val::S32(-1))
        .expect_call("echo-s32", vec![Val::S32(i32::MIN)], Val::S32(i32::MIN))
        .expect_call("echo-s32", vec![Val::S32(i32::MAX)], Val::S32(i32::MAX))
        .expect_call("echo-s64", vec![Val::S64(-1)], Val::S64(-1))
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_echo_lists_of_each_primitive() {
    // list<bool>, list<u8>, list<s32>, list<f64> — matching compjs echo_lists_* tests
    TestCase::new()
        .wit(
            r#"
            package test:prim-lists;
            world prim-lists {
                export echo-bools: func(v: list<bool>) -> list<bool>;
                export echo-u8s: func(v: list<u8>) -> list<u8>;
                export echo-f64s: func(v: list<f64>) -> list<f64>;
            }
        "#,
        )
        .script(
            r#"
            function echoBools(v) { return v; }
            function echoU8s(v) { return v; }
            function echoF64s(v) { return v; }
        "#,
        )
        .stub_wasi()
        .expect_call(
            "echo-bools",
            vec![Val::List(vec![
                Val::Bool(true),
                Val::Bool(false),
                Val::Bool(true),
            ])],
            Val::List(vec![Val::Bool(true), Val::Bool(false), Val::Bool(true)]),
        )
        .expect_call(
            "echo-u8s",
            vec![Val::List(vec![Val::U8(0), Val::U8(127), Val::U8(255)])],
            Val::List(vec![Val::U8(0), Val::U8(127), Val::U8(255)]),
        )
        .expect_call(
            "echo-f64s",
            vec![Val::List(vec![
                Val::Float64(1.0),
                Val::Float64(-0.5),
                Val::Float64(f64::MAX),
            ])],
            Val::List(vec![
                Val::Float64(1.0),
                Val::Float64(-0.5),
                Val::Float64(f64::MAX),
            ]),
        )
        .build()
        .unwrap()
        .run();
}

#[test]
fn test_exported_resource() {
    let dir = tempfile::TempDir::new().unwrap();
    let wit_path = dir.path().join("test.wit");
    std::fs::write(
        &wit_path,
        r#"
        package test:res;

        interface counter-api {
            resource counter {
                constructor(initial: u32);
                increment: func();
                get-value: func() -> u32;
            }
        }

        world resource-test {
            export counter-api;
        }
    "#,
    )
    .unwrap();

    let opts = componentize_qjs::ComponentizeOpts {
        wit_path: &wit_path,
        js_source: r#"
            class Counter {
                constructor(initial) { this.value = initial; }
                increment() { this.value++; }
                getValue() { return this.value; }
            }
            globalThis.counterApi = { Counter };
        "#,
        world_name: None,
        stub_wasi: true,
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let wasm = rt.block_on(componentize_qjs::componentize(&opts)).unwrap();

    // Component builds successfully with resource types
    let engine = common::engine();
    let component = wasmtime::component::Component::new(engine, &wasm).unwrap();

    // Instantiate and call resource methods through the interface
    let mut wasi_builder = wasmtime_wasi::WasiCtxBuilder::new();
    let wasi = wasi_builder.build();
    let table = wasmtime::component::ResourceTable::new();
    let mut store = wasmtime::Store::new(engine, common::WasiCtxState { wasi, table });

    let mut linker = wasmtime::component::Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).unwrap();
    let instance = linker.instantiate(&mut store, &component).unwrap();

    // Navigate into the exported interface
    let iface_idx = instance
        .get_export_index(&mut store, None, "test:res/counter-api")
        .expect("interface export not found");

    // Get constructor
    let ctor_idx = instance
        .get_export_index(&mut store, Some(&iface_idx), "[constructor]counter")
        .expect("[constructor]counter not found");
    let ctor = instance.get_func(&mut store, ctor_idx).unwrap();

    // Call constructor(42)
    let mut results = [Val::Bool(false)];
    ctor.call(&mut store, &[Val::U32(42)], &mut results)
        .unwrap();
    let counter = results[0].clone();

    // Get get-value method
    let get_val_idx = instance
        .get_export_index(&mut store, Some(&iface_idx), "[method]counter.get-value")
        .expect("[method]counter.get-value not found");
    let get_val = instance.get_func(&mut store, get_val_idx).unwrap();

    // Call get-value(counter) => 42
    let mut results = [Val::Bool(false)];
    get_val
        .call(&mut store, std::slice::from_ref(&counter), &mut results)
        .unwrap();
    assert_eq!(results[0], Val::U32(42), "initial value should be 42");

    // Get increment method
    let inc_idx = instance
        .get_export_index(&mut store, Some(&iface_idx), "[method]counter.increment")
        .expect("[method]counter.increment not found");
    let inc = instance.get_func(&mut store, inc_idx).unwrap();

    // Call increment(counter)
    inc.call(&mut store, std::slice::from_ref(&counter), &mut [])
        .unwrap();

    // Verify value is now 43
    let mut results = [Val::Bool(false)];
    get_val
        .call(&mut store, std::slice::from_ref(&counter), &mut results)
        .unwrap();

    assert_eq!(
        results[0],
        Val::U32(43),
        "value should be 43 after increment"
    );
}

#[test]
fn test_static_resource_method_in_interface() {
    let dir = tempfile::TempDir::new().unwrap();
    let wit_path = dir.path().join("test.wit");
    std::fs::write(
        &wit_path,
        r#"
        package test:static-bug;

        interface widget-api {
            resource widget {
                constructor(name: string);
                get-name: func() -> string;
                create-default: static func() -> widget;
            }
        }

        world static-test {
            export widget-api;
        }
        "#,
    )
    .unwrap();

    let opts = componentize_qjs::ComponentizeOpts {
        wit_path: &wit_path,
        js_source: r#"
            class Widget {
                constructor(name) { this.name = name; }
                getName() { return this.name; }
                static createDefault() { return new Widget("default"); }
            }
            globalThis.widgetApi = { Widget };
        "#,
        world_name: None,
        stub_wasi: true,
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let wasm = rt.block_on(componentize_qjs::componentize(&opts)).unwrap();

    let engine = common::engine();
    let component = wasmtime::component::Component::new(engine, &wasm).unwrap();

    let mut wasi_builder = wasmtime_wasi::WasiCtxBuilder::new();
    let wasi = wasi_builder.build();
    let table = wasmtime::component::ResourceTable::new();
    let mut store = wasmtime::Store::new(engine, common::WasiCtxState { wasi, table });

    let mut linker = wasmtime::component::Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).unwrap();
    let instance = linker.instantiate(&mut store, &component).unwrap();

    let iface_idx = instance
        .get_export_index(&mut store, None, "test:static-bug/widget-api")
        .expect("interface export not found");

    // Call the static method — this should work but panics because
    // the runtime looks for "createDefault" in globals instead of the
    // interface object.
    let static_idx = instance
        .get_export_index(
            &mut store,
            Some(&iface_idx),
            "[static]widget.create-default",
        )
        .expect("[static]widget.create-default not found");
    let static_fn = instance.get_func(&mut store, static_idx).unwrap();

    let mut results = [Val::Bool(false)];
    static_fn.call(&mut store, &[], &mut results).unwrap();

    // If we got here, we have a resource handle. Verify it works.
    let get_name_idx = instance
        .get_export_index(&mut store, Some(&iface_idx), "[method]widget.get-name")
        .expect("[method]widget.get-name not found");
    let get_name = instance.get_func(&mut store, get_name_idx).unwrap();

    let mut name_results = [Val::Bool(false)];
    get_name
        .call(&mut store, &results, &mut name_results)
        .unwrap();
    assert_eq!(
        name_results[0],
        Val::String("default".into()),
        "static factory should produce widget with name 'default'"
    );
}

#[tokio::test]
async fn test_async_export_rejection_propagates() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-reject;
            world async-reject {
                export will-throw: async func();
            }
            "#,
        )
        .script(
            r#"
            async function willThrow() {
                throw new Error("this should not be silently swallowed");
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    // The host calls a void async export that throws. The rejection should
    // propagate as an error, not be silently swallowed.
    let result = instance.call_async("will-throw", &[], 0).await;
    assert!(
        result.is_err(),
        "async export that throws should return an error, not Ok"
    );
}

#[test]
fn test_root_level_flags() {
    let result = TestCase::new()
        .wit(
            r#"
            package test:root-flags;
            world root-flags {
                flags permissions {
                    read,
                    write,
                    execute,
                }
                export check: func(p: permissions) -> string;
            }
            "#,
        )
        .script(
            r#"
            function check(p) {
                const parts = [];
                if (p & Permissions.Read) parts.push("read");
                if (p & Permissions.Write) parts.push("write");
                if (p & Permissions.Execute) parts.push("execute");
                return parts.join(",");
            }
            "#,
        )
        .stub_wasi()
        .build();

    let mut instance = result.expect(
        "componentization should succeed for world-level flags — \
         if this fails, partition_imports is dropping root-level types",
    );

    let flags_val = Val::Flags(vec!["read".into(), "execute".into()]);
    let ret = instance.call1("check", &[flags_val]);
    assert_eq!(
        ret,
        Val::String("read,execute".into()),
        "root-level flags should round-trip through the component"
    );
}
