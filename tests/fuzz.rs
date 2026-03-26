//! Property-based stress test using quickcheck.
//!
//! Builds a single component with diverse WIT export types and verifies that
//! repeated invocations with random inputs do not cause unbounded memory growth
//! in the runtime.
//!
//! Uses a two round comparison:
//! 1. Warmup: stabilize caches and atom interning.
//! 2. Round 1: run N ops, GC, snapshot memory `after_round1`.
//! 3. Round 2:  run N more ops, GC, snapshot memory `after_round2`.
//! 4. Assert: `(after_round2 - after_round1)` should be near zero.
mod common;

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use quickcheck::{quickcheck, Arbitrary, Gen, TestResult};
use wasmtime::component::Val;

use componentize_qjs::ComponentizeOpts;

const MAX_SAFE_INT: i64 = (1i64 << 53) - 1;
const MEM_TOLERANCE: i64 = 1024;

#[derive(Debug, Clone)]
struct MemorySnapshot {
    malloc_size: i64,
    memory_used_size: i64,
}

impl MemorySnapshot {
    fn from_val(val: &Val) -> Self {
        let Val::Record(fields) = val else {
            panic!("expected record, got: {val:?}");
        };
        let Val::S64(malloc_size) = &fields[0].1 else {
            panic!("expected s64, got: {:?}", fields[0].1);
        };
        let Val::S64(memory_used_size) = &fields[1].1 else {
            panic!("expected s64, got: {:?}", fields[1].1);
        };
        MemorySnapshot {
            malloc_size: *malloc_size,
            memory_used_size: *memory_used_size,
        }
    }
}

#[derive(Debug, Clone)]
enum Op {
    EchoU8(u8),
    EchoU16(u16),
    EchoU32(u32),
    EchoS32(i32),
    EchoS64(i64),
    EchoU64(u64),
    EchoF64(f64),
    EchoBool(bool),
    EchoChar(char),
    EchoString(String),
    ConcatStrings(String, String),
    EchoBytes(Vec<u8>),
    EchoListU32(Vec<u32>),
    EchoListString(Vec<String>),
    EchoRecord { x: f64, y: f64 },
    EchoTuple(String, u32),
    EchoOptionSome(String),
    EchoOptionNone,
    EchoResultOk(String),
    EchoResultErr(u32),
    EchoVariantCircle(f64),
    EchoVariantRect(f64, f64),
    EchoVariantNone,
    EchoEnum(u8),
    EchoFlags(Vec<String>),
    Accumulate(String),
    ResetAccumulator,
}

fn arbitrary_flags(g: &mut Gen) -> Vec<String> {
    ["read", "write", "execute"]
        .iter()
        .filter(|_| bool::arbitrary(g))
        .map(|n| n.to_string())
        .collect()
}

fn finite_f64(g: &mut Gen) -> f64 {
    loop {
        let v = f64::arbitrary(g);
        if v.is_finite() {
            return v;
        }
    }
}

impl Arbitrary for Op {
    fn arbitrary(g: &mut Gen) -> Self {
        let variant = u8::arbitrary(g) % 27;
        match variant {
            0 => Op::EchoU8(u8::arbitrary(g)),
            1 => Op::EchoU16(u16::arbitrary(g)),
            2 => Op::EchoU32(u32::arbitrary(g)),
            3 => Op::EchoS32(i32::arbitrary(g)),
            4 => Op::EchoS64(i64::arbitrary(g) % (MAX_SAFE_INT + 1)),
            5 => Op::EchoU64(u64::arbitrary(g) % (MAX_SAFE_INT as u64 + 1)),
            6 => Op::EchoF64(finite_f64(g)),
            7 => Op::EchoBool(bool::arbitrary(g)),
            8 => Op::EchoChar(char::arbitrary(g)),
            9 => Op::EchoString(String::arbitrary(g)),
            10 => Op::ConcatStrings(String::arbitrary(g), String::arbitrary(g)),
            11 => Op::EchoBytes(Vec::<u8>::arbitrary(g)),
            12 => Op::EchoListU32(Vec::<u32>::arbitrary(g)),
            13 => Op::EchoListString(Vec::<String>::arbitrary(g)),
            14 => Op::EchoRecord {
                x: finite_f64(g),
                y: finite_f64(g),
            },
            15 => Op::EchoTuple(String::arbitrary(g), u32::arbitrary(g)),
            16 => Op::EchoOptionSome(String::arbitrary(g)),
            17 => Op::EchoOptionNone,
            18 => Op::EchoResultOk(String::arbitrary(g)),
            19 => Op::EchoResultErr(u32::arbitrary(g)),
            20 => Op::EchoVariantCircle(finite_f64(g)),
            21 => Op::EchoVariantRect(finite_f64(g), finite_f64(g)),
            22 => Op::EchoVariantNone,
            23 => Op::EchoEnum(u8::arbitrary(g) % 3),
            24 => Op::EchoFlags(arbitrary_flags(g)),
            25 => Op::Accumulate(String::arbitrary(g)),
            _ => Op::ResetAccumulator,
        }
    }

    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
        match self {
            Op::EchoString(s) => {
                let s = s.clone();
                Box::new(s.shrink().map(Op::EchoString))
            }
            Op::EchoBytes(b) => {
                let b = b.clone();
                Box::new(b.shrink().map(Op::EchoBytes))
            }
            _ => quickcheck::empty_shrinker(),
        }
    }
}

#[derive(Debug, Clone)]
struct Scenario {
    ops: Vec<Op>,
}

impl Arbitrary for Scenario {
    fn arbitrary(g: &mut Gen) -> Self {
        let len = 50 + (usize::arbitrary(g) % 151); // 50..200 ops
        let ops = (0..len).map(|_| Op::arbitrary(g)).collect();
        Scenario { ops }
    }

    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
        let ops = self.ops.clone();
        Box::new(
            ops.shrink()
                .filter(|v| v.len() >= 20)
                .map(|ops| Scenario { ops }),
        )
    }
}

fn warmup_ops() -> Vec<Op> {
    let mut ops = vec![
        Op::EchoU8(42),
        Op::EchoU16(1000),
        Op::EchoU32(42),
        Op::EchoS32(-42),
        Op::EchoS64(-100),
        Op::EchoU64(999),
        Op::EchoF64(2.72),
        Op::EchoBool(true),
        Op::EchoChar('A'),
        Op::EchoString("warmup".into()),
        Op::ConcatStrings("a".into(), "b".into()),
        Op::EchoBytes(vec![1, 2]),
        Op::EchoListU32(vec![1, 2]),
        Op::EchoListString(vec!["x".into()]),
        Op::EchoRecord { x: 1.0, y: 2.0 },
        Op::EchoTuple("w".into(), 1),
        Op::EchoOptionSome("w".into()),
        Op::EchoOptionNone,
        Op::EchoResultOk("ok".into()),
        Op::EchoResultErr(1),
        Op::EchoVariantCircle(1.0),
        Op::EchoVariantNone,
        Op::EchoEnum(0),
        Op::EchoFlags(vec!["read".into(), "write".into()]),
        Op::Accumulate("w".into()),
        Op::ResetAccumulator,
    ];
    for i in 0..30 {
        ops.push(Op::EchoString(format!("warmup-{i}")));
        ops.push(Op::EchoBytes((0..50).collect()));
        ops.push(Op::EchoListString(
            (0..5).map(|j| format!("s{j}")).collect(),
        ));
    }
    ops
}

fn check_mem(
    after_round1: &MemorySnapshot,
    after_round2: &MemorySnapshot,
    label: &str,
    num_ops: usize,
) -> TestResult {
    let malloc_delta = after_round2.malloc_size - after_round1.malloc_size;
    let mem_used_delta = after_round2.memory_used_size - after_round1.memory_used_size;

    if malloc_delta > MEM_TOLERANCE {
        eprintln!(
            "{label}: malloc_size grew by {malloc_delta} bytes in round 2 \
             (round1={}, round2={}, tolerance={MEM_TOLERANCE})",
            after_round1.malloc_size, after_round2.malloc_size
        );
        eprintln!("  ops: {num_ops}");
        return TestResult::failed();
    }

    if mem_used_delta > MEM_TOLERANCE {
        eprintln!(
            "{label}: memory_used_size grew by {mem_used_delta} bytes in round 2 \
             (round1={}, round2={}, tolerance={MEM_TOLERANCE})",
            after_round1.memory_used_size, after_round2.memory_used_size
        );
        return TestResult::failed();
    }

    TestResult::passed()
}

mod fuzz {
    use super::common::ComponentInstance;
    use super::*;

    fn wasm_bytes() -> &'static Vec<u8> {
        static WASM: OnceLock<Vec<u8>> = OnceLock::new();
        WASM.get_or_init(|| {
            let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/wit/fuzz");
            let js = std::fs::read_to_string(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/js/fuzz.js"),
            )
            .expect("failed to read tests/js/fuzz.js");

            let opts = ComponentizeOpts {
                wit_path: &wit,
                js_source: &js,
                world_name: None,
                stub_wasi: true,
                disable_gc: false,
            };

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(componentize_qjs::componentize(&opts)).unwrap()
        })
    }

    fn instance() -> &'static Mutex<ComponentInstance> {
        static INSTANCE: OnceLock<Mutex<ComponentInstance>> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let inst = ComponentInstance::from_wasm(wasm_bytes().clone(), vec![], vec![]).unwrap();
            Mutex::new(inst)
        })
    }

    fn execute_op(inst: &mut ComponentInstance, op: &Op) {
        match op {
            Op::EchoU8(v) => {
                let result = inst.call1("echo-u8", &[Val::U8(*v)]);
                assert_eq!(result, Val::U8(*v));
            }
            Op::EchoU16(v) => {
                let result = inst.call1("echo-u16", &[Val::U16(*v)]);
                assert_eq!(result, Val::U16(*v));
            }
            Op::EchoU32(v) => {
                let result = inst.call1("echo-u32", &[Val::U32(*v)]);
                assert_eq!(result, Val::U32(*v));
            }
            Op::EchoS32(v) => {
                let result = inst.call1("echo-s32", &[Val::S32(*v)]);
                assert_eq!(result, Val::S32(*v));
            }
            Op::EchoS64(v) => {
                let result = inst.call1("echo-s64", &[Val::S64(*v)]);
                assert_eq!(result, Val::S64(*v));
            }
            Op::EchoU64(v) => {
                let result = inst.call1("echo-u64", &[Val::U64(*v)]);
                assert_eq!(result, Val::U64(*v));
            }
            Op::EchoF64(v) => {
                let result = inst.call1("echo-f64", &[Val::Float64(*v)]);
                assert_eq!(result, Val::Float64(*v));
            }
            Op::EchoBool(v) => {
                let result = inst.call1("echo-bool", &[Val::Bool(*v)]);
                assert_eq!(result, Val::Bool(*v));
            }
            Op::EchoChar(v) => {
                let result = inst.call1("echo-char", &[Val::Char(*v)]);
                assert_eq!(result, Val::Char(*v));
            }
            Op::EchoString(v) => {
                let result = inst.call1("echo-string", &[Val::String(v.clone())]);
                assert_eq!(result, Val::String(v.clone()));
            }
            Op::ConcatStrings(a, b) => {
                let result = inst.call1(
                    "concat-strings",
                    &[Val::String(a.clone()), Val::String(b.clone())],
                );
                assert_eq!(result, Val::String(format!("{a}{b}")));
            }
            Op::EchoBytes(v) => {
                let list: Vec<Val> = v.iter().map(|b| Val::U8(*b)).collect();
                let result = inst.call1("echo-bytes", &[Val::List(list.clone())]);
                assert_eq!(result, Val::List(list));
            }
            Op::EchoListU32(v) => {
                let list: Vec<Val> = v.iter().map(|n| Val::U32(*n)).collect();
                let result = inst.call1("echo-list-u32", &[Val::List(list.clone())]);
                assert_eq!(result, Val::List(list));
            }
            Op::EchoListString(v) => {
                let list: Vec<Val> = v.iter().map(|s| Val::String(s.clone())).collect();
                let result = inst.call1("echo-list-string", &[Val::List(list.clone())]);
                assert_eq!(result, Val::List(list));
            }
            Op::EchoRecord { x, y } => {
                let record = Val::Record(vec![
                    ("x".into(), Val::Float64(*x)),
                    ("y".into(), Val::Float64(*y)),
                ]);
                let result = inst.call1("echo-record", std::slice::from_ref(&record));
                assert_eq!(result, record);
            }
            Op::EchoTuple(s, n) => {
                let tuple = Val::Tuple(vec![Val::String(s.clone()), Val::U32(*n)]);
                let result = inst.call1("echo-tuple", std::slice::from_ref(&tuple));
                assert_eq!(result, tuple);
            }
            Op::EchoOptionSome(s) => {
                let opt = Val::Option(Some(Box::new(Val::String(s.clone()))));
                let result = inst.call1("echo-option-string", std::slice::from_ref(&opt));
                assert_eq!(result, opt);
            }
            Op::EchoOptionNone => {
                let result = inst.call1("echo-option-string", &[Val::Option(None)]);
                assert_eq!(result, Val::Option(None));
            }
            Op::EchoResultOk(s) => {
                let res = Val::Result(Ok(Some(Box::new(Val::String(s.clone())))));
                let result = inst.call1("echo-result", std::slice::from_ref(&res));
                assert_eq!(result, res);
            }
            Op::EchoResultErr(n) => {
                let res = Val::Result(Err(Some(Box::new(Val::U32(*n)))));
                let result = inst.call1("echo-result", std::slice::from_ref(&res));
                assert_eq!(result, res);
            }
            Op::EchoVariantCircle(r) => {
                let variant = Val::Variant("circle".into(), Some(Box::new(Val::Float64(*r))));
                let result = inst.call1("echo-variant", std::slice::from_ref(&variant));
                assert_eq!(result, variant);
            }
            Op::EchoVariantRect(w, h) => {
                let variant = Val::Variant(
                    "rectangle".into(),
                    Some(Box::new(Val::Tuple(vec![
                        Val::Float64(*w),
                        Val::Float64(*h),
                    ]))),
                );
                let result = inst.call1("echo-variant", std::slice::from_ref(&variant));
                assert_eq!(result, variant);
            }
            Op::EchoVariantNone => {
                let variant = Val::Variant("none".into(), None);
                let result = inst.call1("echo-variant", std::slice::from_ref(&variant));
                assert_eq!(result, variant);
            }
            Op::EchoEnum(n) => {
                let names = ["red", "green", "blue"];
                let name = names[*n as usize];
                let result = inst.call1("echo-enum", &[Val::Enum(name.into())]);
                assert_eq!(result, Val::Enum(name.into()));
            }
            Op::EchoFlags(flags) => {
                let val = Val::Flags(flags.to_vec());
                let result = inst.call1("echo-flags", std::slice::from_ref(&val));
                assert_eq!(result, val);
            }
            Op::Accumulate(s) => {
                inst.call1("accumulate", &[Val::String(s.clone())]);
            }
            Op::ResetAccumulator => {
                inst.call("reset-accumulator", &[], 0);
            }
        }
    }

    fn run(inst: &mut ComponentInstance, ops: &[Op]) -> MemorySnapshot {
        for op in ops {
            execute_op(inst, op);
        }
        inst.call("reset-accumulator", &[], 0);
        inst.call("run-gc", &[], 0);
        let val = inst.call1("get-memory-usage", &[]);
        MemorySnapshot::from_val(&val)
    }

    quickcheck! {
        fn qc_mem_profile(scenario: Scenario) -> TestResult {
            let mut inst = instance().lock().unwrap();

            for op in warmup_ops() {
                execute_op(&mut inst, &op);
            }
            inst.call("run-gc", &[], 0);

            let after_round1 = run(&mut inst, &scenario.ops);
            let after_round2 = run(&mut inst, &scenario.ops);

            check_mem(&after_round1, &after_round2, "SYNC", scenario.ops.len())
        }
    }
}

mod fuzz_async {
    use super::common::AsyncComponentInstance;
    use super::*;

    fn wasm_bytes() -> &'static Vec<u8> {
        static WASM: OnceLock<Vec<u8>> = OnceLock::new();
        WASM.get_or_init(|| {
            let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/wit/async-fuzz");
            let js = std::fs::read_to_string(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/js/async-fuzz.js"),
            )
            .expect("failed to read tests/js/async-fuzz.js");

            let opts = ComponentizeOpts {
                wit_path: &wit,
                js_source: &js,
                world_name: None,
                stub_wasi: false,
                disable_gc: false,
            };

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(componentize_qjs::componentize(&opts)).unwrap()
        })
    }

    fn instance() -> &'static Mutex<AsyncComponentInstance> {
        static INSTANCE: OnceLock<Mutex<AsyncComponentInstance>> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            let inst = rt
                .block_on(AsyncComponentInstance::from_wasm(
                    wasm_bytes().clone(),
                    vec![],
                ))
                .unwrap();
            Mutex::new(inst)
        })
    }

    fn with_runtime<F, T>(f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    async fn execute_op(inst: &mut AsyncComponentInstance, op: &Op) {
        match op {
            Op::EchoU8(v) => {
                let result = inst.call1_async("echo-u8", &[Val::U8(*v)]).await.unwrap();
                assert_eq!(result, Val::U8(*v));
            }
            Op::EchoU16(v) => {
                let result = inst.call1_async("echo-u16", &[Val::U16(*v)]).await.unwrap();
                assert_eq!(result, Val::U16(*v));
            }
            Op::EchoU32(v) => {
                let result = inst.call1_async("echo-u32", &[Val::U32(*v)]).await.unwrap();
                assert_eq!(result, Val::U32(*v));
            }
            Op::EchoS32(v) => {
                let result = inst.call1_async("echo-s32", &[Val::S32(*v)]).await.unwrap();
                assert_eq!(result, Val::S32(*v));
            }
            Op::EchoS64(v) => {
                let result = inst.call1_async("echo-s64", &[Val::S64(*v)]).await.unwrap();
                assert_eq!(result, Val::S64(*v));
            }
            Op::EchoU64(v) => {
                let result = inst.call1_async("echo-u64", &[Val::U64(*v)]).await.unwrap();
                assert_eq!(result, Val::U64(*v));
            }
            Op::EchoF64(v) => {
                let result = inst
                    .call1_async("echo-f64", &[Val::Float64(*v)])
                    .await
                    .unwrap();
                assert_eq!(result, Val::Float64(*v));
            }
            Op::EchoBool(v) => {
                let result = inst
                    .call1_async("echo-bool", &[Val::Bool(*v)])
                    .await
                    .unwrap();
                assert_eq!(result, Val::Bool(*v));
            }
            Op::EchoChar(v) => {
                let result = inst
                    .call1_async("echo-char", &[Val::Char(*v)])
                    .await
                    .unwrap();
                assert_eq!(result, Val::Char(*v));
            }
            Op::EchoString(v) => {
                let result = inst
                    .call1_async("echo-string", &[Val::String(v.clone())])
                    .await
                    .unwrap();
                assert_eq!(result, Val::String(v.clone()));
            }
            Op::ConcatStrings(a, b) => {
                let result = inst
                    .call1_async(
                        "concat-strings",
                        &[Val::String(a.clone()), Val::String(b.clone())],
                    )
                    .await
                    .unwrap();
                assert_eq!(result, Val::String(format!("{a}{b}")));
            }
            Op::EchoBytes(v) => {
                let list: Vec<Val> = v.iter().map(|b| Val::U8(*b)).collect();
                let result = inst
                    .call1_async("echo-bytes", &[Val::List(list.clone())])
                    .await
                    .unwrap();
                assert_eq!(result, Val::List(list));
            }
            Op::EchoListU32(v) => {
                let list: Vec<Val> = v.iter().map(|n| Val::U32(*n)).collect();
                let result = inst
                    .call1_async("echo-list-u32", &[Val::List(list.clone())])
                    .await
                    .unwrap();
                assert_eq!(result, Val::List(list));
            }
            Op::EchoListString(v) => {
                let list: Vec<Val> = v.iter().map(|s| Val::String(s.clone())).collect();
                let result = inst
                    .call1_async("echo-list-string", &[Val::List(list.clone())])
                    .await
                    .unwrap();
                assert_eq!(result, Val::List(list));
            }
            Op::EchoRecord { x, y } => {
                let record = Val::Record(vec![
                    ("x".into(), Val::Float64(*x)),
                    ("y".into(), Val::Float64(*y)),
                ]);
                let result = inst
                    .call1_async("echo-record", std::slice::from_ref(&record))
                    .await
                    .unwrap();
                assert_eq!(result, record);
            }
            Op::EchoTuple(s, n) => {
                let tuple = Val::Tuple(vec![Val::String(s.clone()), Val::U32(*n)]);
                let result = inst
                    .call1_async("echo-tuple", std::slice::from_ref(&tuple))
                    .await
                    .unwrap();
                assert_eq!(result, tuple);
            }
            Op::EchoOptionSome(s) => {
                let opt = Val::Option(Some(Box::new(Val::String(s.clone()))));
                let result = inst
                    .call1_async("echo-option-string", std::slice::from_ref(&opt))
                    .await
                    .unwrap();
                assert_eq!(result, opt);
            }
            Op::EchoOptionNone => {
                let result = inst
                    .call1_async("echo-option-string", &[Val::Option(None)])
                    .await
                    .unwrap();
                assert_eq!(result, Val::Option(None));
            }
            Op::EchoResultOk(s) => {
                let res = Val::Result(Ok(Some(Box::new(Val::String(s.clone())))));
                let result = inst
                    .call1_async("echo-result", std::slice::from_ref(&res))
                    .await
                    .unwrap();
                assert_eq!(result, res);
            }
            Op::EchoResultErr(n) => {
                let res = Val::Result(Err(Some(Box::new(Val::U32(*n)))));
                let result = inst
                    .call1_async("echo-result", std::slice::from_ref(&res))
                    .await
                    .unwrap();
                assert_eq!(result, res);
            }
            Op::EchoVariantCircle(r) => {
                let variant = Val::Variant("circle".into(), Some(Box::new(Val::Float64(*r))));
                let result = inst
                    .call1_async("echo-variant", std::slice::from_ref(&variant))
                    .await
                    .unwrap();
                assert_eq!(result, variant);
            }
            Op::EchoVariantRect(w, h) => {
                let variant = Val::Variant(
                    "rectangle".into(),
                    Some(Box::new(Val::Tuple(vec![
                        Val::Float64(*w),
                        Val::Float64(*h),
                    ]))),
                );
                let result = inst
                    .call1_async("echo-variant", std::slice::from_ref(&variant))
                    .await
                    .unwrap();
                assert_eq!(result, variant);
            }
            Op::EchoVariantNone => {
                let variant = Val::Variant("none".into(), None);
                let result = inst
                    .call1_async("echo-variant", std::slice::from_ref(&variant))
                    .await
                    .unwrap();
                assert_eq!(result, variant);
            }
            Op::EchoEnum(n) => {
                let names = ["red", "green", "blue"];
                let name = names[*n as usize];
                let result = inst
                    .call1_async("echo-enum", &[Val::Enum(name.into())])
                    .await
                    .unwrap();
                assert_eq!(result, Val::Enum(name.into()));
            }
            Op::EchoFlags(flags) => {
                let val = Val::Flags(flags.to_vec());
                let result = inst
                    .call1_async("echo-flags", std::slice::from_ref(&val))
                    .await
                    .unwrap();
                assert_eq!(result, val);
            }
            Op::Accumulate(s) => {
                inst.call1_async("accumulate", &[Val::String(s.clone())])
                    .await
                    .unwrap();
            }
            Op::ResetAccumulator => {
                inst.call_async("reset-accumulator", &[], 0).await.unwrap();
            }
        }
    }

    async fn run(inst: &mut AsyncComponentInstance, ops: &[Op]) -> MemorySnapshot {
        for op in ops {
            execute_op(inst, op).await;
        }
        inst.call_async("reset-accumulator", &[], 0).await.unwrap();
        inst.call_async("run-gc", &[], 0).await.unwrap();
        let val = inst.call1_async("get-memory-usage", &[]).await.unwrap();
        MemorySnapshot::from_val(&val)
    }

    quickcheck! {
        fn qc_mem_profile(scenario: Scenario) -> TestResult {
            let mut inst = instance().lock().unwrap();

            with_runtime(async {
                for op in warmup_ops() {
                    execute_op(&mut inst, &op).await;
                }
                inst.call_async("run-gc", &[], 0).await.unwrap();

                let after_round1 = run(&mut inst, &scenario.ops).await;
                let after_round2 = run(&mut inst, &scenario.ops).await;

                check_mem(&after_round1, &after_round2, "ASYNC", scenario.ops.len())
            })
        }
    }
}
