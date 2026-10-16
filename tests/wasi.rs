//! WASI integration tests for componentize-qjs
mod common;

use std::path::PathBuf;

use wasmtime::component::Val;

use common::{TestCase, wasi_wit_dir};

#[test]
fn test_wasi_random() {
    let mut inst = TestCase::new()
        .wit_dir(wasi_wit_dir())
        .world("wasi-random")
        .script(
            r#"
            import random from "wasi:random/random@0.2.12";

            export function getRandomU64() { return random.getRandomU64(); }
            export function getRandomBytes(len) { return random.getRandomBytes(BigInt(len)); }
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
fn test_wasi_named_imports() {
    let mut inst = TestCase::new()
        .wit_dir(wasi_wit_dir())
        .world("wasi-random")
        .script(
            r#"
            import {
                getRandomBytes as randomBytes,
                getRandomU64 as randomU64,
            } from "wasi:random/random@0.2.12";

            export function getRandomU64() { return randomU64(); }
            export function getRandomBytes(len) { return randomBytes(BigInt(len)); }
        "#,
        )
        .build()
        .expect("should build wasi-random component with named imports");

    let val = inst.call1("get-random-u64", &[]);
    assert!(matches!(val, Val::U64(_)), "Expected u64, got: {:?}", val);

    let bytes = inst.call1("get-random-bytes", &[Val::U32(8)]);
    match &bytes {
        Val::List(items) => assert_eq!(items.len(), 8, "Expected 8 random bytes"),
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
            import clock from "wasi:clocks/monotonic-clock@0.2.12";

            export function getTimeNs() { return clock.now(); }
            export function elapsedNs() {
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
            import env from "wasi:cli/environment@0.2.12";

            export function getEnvVars() { return env.getEnvironment(); }
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

#[test]
fn test_wasi_stdio() {
    let mut inst = TestCase::new()
        .wit_dir(wasi_wit_dir())
        .world("wasi-stdio")
        .stdin("hello from stdin")
        .script(
            r#"
            import stdin from "wasi:cli/stdin@0.2.12";
            import stdout from "wasi:cli/stdout@0.2.12";

            export function echoStdinToStdout() {
                const input = stdin.getStdin();
                const output = stdout.getStdout();

                while (true) {
                    let chunk;
                    try {
                        chunk = input.blockingRead(4096n);
                    } catch (e) {
                        if (e && e.payload && e.payload.tag === "closed") {
                            break;
                        }
                        throw e;
                    }

                    output.blockingWriteAndFlush(chunk);
                }

                return;
            }
        "#,
        )
        .build()
        .expect("should build wasi-stdio component");

    let result = inst.call1("echo-stdin-to-stdout", &[]);
    assert_eq!(result, Val::Result(Ok(None)));
    assert_eq!(inst.stdout_bytes(), b"hello from stdin");
    assert!(
        inst.parts().1.data().table.is_empty(),
        "stdio resources should be released before post-return"
    );
}

#[test]
fn test_wasi_resources_finalized_during_result_lowering() {
    let mut inst = TestCase::new()
        .wit_dir(wasi_wit_dir())
        .world("wasi-resource-cleanup")
        .script(
            r#"
            import stdin from "wasi:cli/stdin@0.2.12";

            export function makeResult() {
                return { value: 7, extra: stdin.getStdin() };
            }

            export function flush() {}
        "#,
        )
        .build()
        .expect("should build wasi-resource-cleanup component");

    // Each next call must release the discarded resource before opening another stream.
    inst.parts().1.data_mut().table.set_max_capacity(1);

    for _ in 0..2 {
        assert_eq!(
            inst.call1("make-result", &[]),
            Val::Record(vec![("value".into(), Val::U32(7))])
        );
    }

    inst.call("flush", &[], 0);
    assert!(
        inst.parts().1.data().table.is_empty(),
        "resources finalized during result lowering should be released on the next entry"
    );
}

#[tokio::test]
async fn test_wasi_0_3_stdio_example() {
    let wit_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/wasi-stdio");

    let mut inst = TestCase::new()
        .wit_dir(wit_path)
        .world("wasi-stdio")
        .stdin("hello from wasi 0.3")
        .script(include_str!("../examples/wasi-stdio/echo.js"))
        .build_async()
        .await
        .expect("should build WASI 0.3 stdio example");

    {
        let (instance, store) = inst.parts();
        let iface_idx = instance
            .get_export_index(&mut *store, None, "wasi:cli/run@0.3.0")
            .expect("wasi:cli/run export not found");
        let run_idx = instance
            .get_export_index(&mut *store, Some(&iface_idx), "run")
            .expect("run export not found");
        let run = instance.get_func(&mut *store, run_idx).unwrap();

        let mut results = [Val::Bool(false)];
        run.call_async(&mut *store, &[], &mut results)
            .await
            .expect("WASI 0.3 stdio run should succeed");
        assert_eq!(results[0], Val::Result(Ok(None)));
    }

    assert_eq!(inst.stdout_bytes(), b"hello from wasi 0.3");
}

#[cfg(feature = "component-model-async")]
#[tokio::test]
async fn test_snapshot_preserves_async_imports_and_shared_resources() {
    let directory = tempfile::tempdir().unwrap();
    let io = directory.path().join("deps/io");
    std::fs::create_dir_all(&io).unwrap();
    std::fs::write(
        io.join("package.wit"),
        include_str!("wit/deps/wasi-io-0.2.12/package.wit"),
    )
    .unwrap();
    for package in ["filesystem", "clocks", "cli", "random", "sockets"] {
        let dep = directory.path().join("deps").join(package);
        std::fs::create_dir_all(&dep).unwrap();
        std::fs::copy(
            wasi_wit_dir().join(format!("deps/wasi-{package}-0.2.12/package.wit")),
            dep.join("package.wit"),
        )
        .unwrap();
    }
    std::fs::write(directory.path().join("test.wit"), r#"
        package test:snapshot-aliases;
        interface host {
            use wasi:io/streams@0.2.12.{input-stream, output-stream};
            use wasi:io/poll@0.2.12.{pollable};
            use wasi:io/error@0.2.12.{error};
            use wasi:filesystem/types@0.2.12.{descriptor, directory-entry-stream};
            inspect: async func(input: borrow<input-stream>) -> u64;
            filesystem: func(file: borrow<descriptor>, entries: borrow<directory-entry-stream>);
            unused: async func(input: borrow<input-stream>, output: borrow<output-stream>, poll: borrow<pollable>, failure: borrow<error>);
        }
        world test {
            import wasi:io/streams@0.2.12;
            import wasi:io/poll@0.2.12;
            import wasi:io/error@0.2.12;
            import wasi:filesystem/types@0.2.12;
            import wasi:cli/stdin@0.2.12;
            import host;
            import transform: async func(value: u64) -> u64;
            export run: async func() -> u64;
        }
    "#).unwrap();
    let component = TestCase::new()
        .wit_dir(directory.path())
        .world("test")
        .script(
            r#"
            import { inspect } from "test:snapshot-aliases/host";
            import { getStdin } from "wasi:cli/stdin@0.2.12";
            export async function run() {
                const input = getStdin();
                try { return await transform(await inspect(input)); }
                finally { input.drop(); }
            }
        "#,
        )
        .compile_async()
        .await
        .expect(
            "snapshot linker must preserve asynchronous functions and WASI resource identities",
        );

    use common::WasiCtxState;
    use wasmtime::Store;
    use wasmtime::component::{Linker, Resource, ResourceTable};
    use wasmtime_wasi::{WasiCtxBuilder, p2::DynInputStream};

    let mut linker = Linker::<WasiCtxState>::new(component.engine());
    wasmtime_wasi::p2::add_to_linker_async(&mut linker).unwrap();
    let mut host = linker.instance("test:snapshot-aliases/host").unwrap();
    host.func_wrap_concurrent(
        "inspect",
        |accessor, (input,): (Resource<DynInputStream>,)| {
            Box::pin(async move {
                accessor.with(|mut store| store.data_mut().table.get(&input).map(|_| ()))?;
                // Force suspension before returning a value beyond Number's precision.
                tokio::task::yield_now().await;
                Ok(((1u64 << 63) + 7,))
            })
        },
    )
    .unwrap();
    host.func_new("filesystem", |_, _, _, _| {
        Err(wasmtime::Error::msg("unused"))
    })
    .unwrap();
    host.func_new_concurrent("unused", |_, _, _, _| {
        Box::pin(async { Err(wasmtime::Error::msg("unused")) })
    })
    .unwrap();
    linker
        .root()
        .func_wrap_concurrent("transform", |_, (value,): (u64,)| {
            Box::pin(async move { Ok((value,)) })
        })
        .unwrap();
    let mut store = Store::new(
        component.engine(),
        WasiCtxState {
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
        },
    );
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    let run = instance
        .get_typed_func::<(), (u64,)>(&mut store, "run")
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            run.call_async(&mut store, ()).await.unwrap(),
            ((1u64 << 63) + 7,)
        );
        assert!(
            store.data().table.is_empty(),
            "borrowed stdin must be released"
        );
    }
}
