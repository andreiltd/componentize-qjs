//! WASI integration tests for componentize-qjs
mod common;

#[cfg(feature = "component-model-async")]
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

#[cfg(feature = "component-model-async")]
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

/// Imported handles preserve transfer, loan, expiry, and disposal rules.
#[cfg(feature = "component-model-async")]
#[tokio::test]
async fn test_wasi_imported_resource_lifecycle() {
    let mut inst = TestCase::new()
        .wit_dir(wasi_wit_dir())
        .world("wasi-resource-lifecycle")
        .stdin("loan")
        .script(
            r#"
            import stdin from "wasi:cli/stdin@0.2.12";
            import stdout from "wasi:cli/stdout@0.2.12";

            let transferred;
            let borrowed;

            function expectTypeError(action, message) {
                try {
                    action();
                } catch (error) {
                    if (error instanceof TypeError && error.message === message) {
                        return;
                    }
                    throw error;
                }
                throw new Error(`expected TypeError: ${message}`);
            }

            function makeOwned() {
                transferred = stdin.getStdin();
                return transferred;
            }

            async function readBorrowed(stream) {
                borrowed = stream;
                expectTypeError(() => stream.drop(), "cannot dispose a borrowed resource");
                return stream.blockingRead(4n);
            }

            function checkInvalidStates() {
                expectTypeError(
                    () => transferred.blockingRead(1n),
                    "resource ownership has already been transferred"
                );
                expectTypeError(
                    () => borrowed.blockingRead(1n),
                    "borrowed resource has expired"
                );
                transferred.drop();
                borrowed.drop();
                return true;
            }

            function checkOutgoingLoan() {
                const output = stdout.getStdout();
                let checked = false;
                const bytes = [120];
                Object.defineProperty(bytes, 0, {
                    get() {
                        expectTypeError(
                            () => output.drop(),
                            "cannot dispose a resource while it is borrowed"
                        );
                        checked = true;
                        return 120;
                    }
                });
                output.blockingWriteAndFlush(bytes);
                output.drop();
                if (!checked) throw new Error("outgoing loan was not checked");
            }

            function disposeOwned(stream) {
                stream.drop();
                stream.drop();
                expectTypeError(() => stream.blockingRead(1n), "resource has been disposed");
                return true;
            }

            export const resourceLifecycle = {
                makeOwned,
                readBorrowed,
                checkInvalidStates,
                checkOutgoingLoan,
                disposeOwned,
            };
            "#,
        )
        .build_async()
        .await
        .expect("should build imported-resource lifecycle component");

    let (instance, store) = inst.parts();
    let interface = instance
        .get_export_index(&mut *store, None, "test:wasi/resource-lifecycle")
        .expect("resource-lifecycle interface not found");
    let [
        make_owned,
        read_borrowed,
        check_invalid_states,
        check_outgoing_loan,
        dispose_owned,
    ] = [
        "make-owned",
        "read-borrowed",
        "check-invalid-states",
        "check-outgoing-loan",
        "dispose-owned",
    ]
    .map(|name| {
        let index = instance
            .get_export_index(&mut *store, Some(&interface), name)
            .expect("resource-lifecycle function not found");
        instance.get_func(&mut *store, index).unwrap()
    });

    let mut results = [Val::Bool(false)];
    make_owned
        .call_async(&mut *store, &[], &mut results)
        .await
        .unwrap();
    let stream = results[0].clone();
    read_borrowed
        .call_async(&mut *store, std::slice::from_ref(&stream), &mut results)
        .await
        .unwrap();
    assert_eq!(
        results[0],
        Val::List(b"loan".iter().copied().map(Val::U8).collect())
    );
    check_invalid_states
        .call_async(&mut *store, &[], &mut results)
        .await
        .unwrap();
    assert_eq!(results[0], Val::Bool(true));
    assert!(
        !store.data().table.is_empty(),
        "ending a borrow must not destroy its owner"
    );

    check_outgoing_loan
        .call_async(&mut *store, &[], &mut [])
        .await
        .unwrap();
    dispose_owned
        .call_async(&mut *store, &[stream], &mut results)
        .await
        .unwrap();
    assert_eq!(results[0], Val::Bool(true));
    assert!(
        store.data().table.is_empty(),
        "explicit disposal should release every owned handle"
    );
    assert_eq!(inst.stdout_bytes(), b"x");
}
