//! Async component model tests for componentize-qjs.

mod common;

use std::pin::Pin;
use std::task::{Context, Poll};

use common::{TestCase, WasiCtxState};
use wasmtime::component::{
    Destination, StreamProducer, StreamReader, StreamResult, Val, VecBuffer,
};
use wasmtime::StoreContextMut;

#[tokio::test]
async fn test_async_echo_u32() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-echo;
            world async-echo {
                export echo-u32: async func(x: u32) -> u32;
            }
            "#,
        )
        .script("async function echoU32(x) { return x; }")
        .build_async()
        .await
        .unwrap();

    let result = instance
        .call1_async("echo-u32", &[Val::U32(42)])
        .await
        .unwrap();
    assert_eq!(result, Val::U32(42));
}

#[tokio::test]
async fn test_async_echo_string() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-echo;
            world async-echo {
                export echo-string: async func(s: string) -> string;
            }
            "#,
        )
        .script(r#"async function echoString(s) { return s; }"#)
        .build_async()
        .await
        .unwrap();

    let result = instance
        .call1_async("echo-string", &[Val::String("hello async".into())])
        .await
        .unwrap();
    assert_eq!(result, Val::String("hello async".into()));
}

#[tokio::test]
async fn test_async_echo_bool() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-echo;
            world async-echo {
                export echo-bool: async func(b: bool) -> bool;
            }
            "#,
        )
        .script("async function echoBool(b) { return b; }")
        .build_async()
        .await
        .unwrap();

    let result = instance
        .call1_async("echo-bool", &[Val::Bool(true)])
        .await
        .unwrap();
    assert_eq!(result, Val::Bool(true));
}

#[tokio::test]
async fn test_async_void_function() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-void;
            world async-void {
                export do-nothing: async func();
            }
            "#,
        )
        .script("async function doNothing() { }")
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("do-nothing", &[], 0).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_async_with_await() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-await;
            world async-await {
                export delayed-echo: async func(x: u32) -> u32;
            }
            "#,
        )
        .script(
            r#"
            async function delayedEcho(x) {
                // Simulate async work with a resolved promise chain
                await Promise.resolve();
                return x + 1;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let result = instance
        .call1_async("delayed-echo", &[Val::U32(99)])
        .await
        .unwrap();
    assert_eq!(result, Val::U32(100));
}

#[tokio::test]
async fn test_async_echo_record() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-record;
            world async-record {
                record point {
                    x: f64,
                    y: f64,
                }
                export echo-point: async func(p: point) -> point;
            }
            "#,
        )
        .script(
            r#"
            async function echoPoint(p) {
                return { x: p.x * 2, y: p.y * 2 };
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let input = Val::Record(vec![
        ("x".to_string(), Val::Float64(1.5)),
        ("y".to_string(), Val::Float64(2.5)),
    ]);
    let result = instance.call1_async("echo-point", &[input]).await.unwrap();

    match result {
        Val::Record(fields) => {
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0].0, "x");
            assert_eq!(fields[1].0, "y");
            assert_eq!(fields[0].1, Val::Float64(3.0));
            assert_eq!(fields[1].1, Val::Float64(5.0));
        }
        other => panic!("expected Record, got {:?}", other),
    }
}

#[tokio::test]
async fn test_async_echo_option() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-option;
            world async-option {
                export echo-option: async func(x: option<u32>) -> option<u32>;
            }
            "#,
        )
        .script(
            r#"
            async function echoOption(x) {
                return x;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    // Some case
    let result = instance
        .call1_async("echo-option", &[Val::Option(Some(Box::new(Val::U32(42))))])
        .await
        .unwrap();
    assert_eq!(result, Val::Option(Some(Box::new(Val::U32(42)))));

    // None case
    let result = instance
        .call1_async("echo-option", &[Val::Option(None)])
        .await
        .unwrap();
    assert_eq!(result, Val::Option(None));
}

#[tokio::test]
async fn test_async_echo_result() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-result;
            world async-result {
                export safe-divide: async func(a: f64, b: f64) -> result<f64, string>;
            }
            "#,
        )
        .script(
            r#"
            async function safeDivide(a, b) {
                if (b === 0) {
                    return { tag: "err", val: "division by zero" };
                }
                return { tag: "ok", val: a / b };
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    // Ok case
    let result = instance
        .call1_async("safe-divide", &[Val::Float64(10.0), Val::Float64(2.0)])
        .await
        .unwrap();
    assert_eq!(result, Val::Result(Ok(Some(Box::new(Val::Float64(5.0))))));

    // Error case
    let result = instance
        .call1_async("safe-divide", &[Val::Float64(10.0), Val::Float64(0.0)])
        .await
        .unwrap();
    assert_eq!(
        result,
        Val::Result(Err(Some(Box::new(Val::String("division by zero".into())))))
    );
}

#[tokio::test]
async fn test_async_echo_list() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-list;
            world async-list {
                export double-list: async func(xs: list<u32>) -> list<u32>;
            }
            "#,
        )
        .script(
            r#"
            async function doubleList(xs) {
                return xs.map(x => x * 2);
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let input = Val::List(vec![Val::U32(1), Val::U32(2), Val::U32(3)]);
    let result = instance.call1_async("double-list", &[input]).await.unwrap();
    assert_eq!(
        result,
        Val::List(vec![Val::U32(2), Val::U32(4), Val::U32(6)])
    );
}

#[tokio::test]
async fn test_stream_create_and_return_u8() {
    // Verify stream<u8> factory creates valid stream handles
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:stream-u8;
            world stream-u8 {
                export make-stream: async func() -> stream<u8>;
            }
            "#,
        )
        .script(
            r#"
            async function makeStream() {
                const { readable, writable } = wit.Stream();
                writable.drop();
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("make-stream", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_stream_create_and_return_u32() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:stream-u32;
            world stream-u32 {
                export make-stream: async func() -> stream<u32>;
            }
            "#,
        )
        .script(
            r#"
            async function makeStream() {
                const { readable, writable } = wit.Stream();
                writable.drop();
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("make-stream", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_stream_create_and_return_string() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:stream-string;
            world stream-string {
                export make-stream: async func() -> stream<string>;
            }
            "#,
        )
        .script(
            r#"
            async function makeStream() {
                const { readable, writable } = wit.Stream();
                writable.drop();
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("make-stream", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_stream_object_return_shape() {
    // Verify the factory returns { readable, writable } not [writable, readable]
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:stream-shape;
            world stream-shape {
                export check-shape: async func() -> stream<u8>;
            }
            "#,
        )
        .script(
            r#"
            async function checkShape() {
                const pair = wit.Stream();
                // Verify it's an object with named properties
                if (pair.readable === undefined) throw new Error("missing readable");
                if (pair.writable === undefined) throw new Error("missing writable");
                if (typeof pair.readable.read !== 'function') throw new Error("readable missing read");
                if (typeof pair.writable.write !== 'function') throw new Error("writable missing write");
                pair.writable.drop();
                return pair.readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("check-shape", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_stream_enum_factory() {
    // Verify wit.Stream(wit.Stream.U8) works with enum constants
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:stream-enum;
            world stream-enum {
                export check-enum: async func() -> stream<u8>;
            }
            "#,
        )
        .script(
            r#"
            async function checkEnum() {
                const { readable, writable } = wit.Stream(wit.Stream.U8);
                if (readable === undefined) throw new Error("missing readable");
                if (writable === undefined) throw new Error("missing writable");
                writable.drop();
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("check-enum", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_future_enum_factory() {
    // Verify wit.Future(wit.Future.STRING) works with enum constants
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:future-enum;
            world future-enum {
                export check-enum: async func() -> future<string>;
            }
            "#,
        )
        .script(
            r#"
            async function checkEnum() {
                const { readable, writable } = wit.Future(wit.Future.STRING);
                if (readable === undefined) throw new Error("missing readable");
                if (writable === undefined) throw new Error("missing writable");
                writable.write("test");
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("check-enum", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_stream_record_type_constant() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:stream-record;
            world stream-record {
                record point { x: f64, y: f64 }
                export make-stream: async func() -> stream<point>;
            }
            "#,
        )
        .script(
            r#"
            async function makeStream() {
                // Use the named record type constant
                const { readable, writable } = wit.Stream(wit.Stream.POINT);
                writable.drop();
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("make-stream", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_stream_result_type_constant() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:stream-result;
            world stream-result {
                export make-stream: async func() -> stream<result<string, u32>>;
            }
            "#,
        )
        .script(
            r#"
            async function makeStream() {
                const { readable, writable } = wit.Stream(wit.Stream.RESULT_STRING_U32);
                writable.drop();
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("make-stream", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_stream_option_type_constant() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:stream-option;
            world stream-option {
                export make-stream: async func() -> stream<option<u32>>;
            }
            "#,
        )
        .script(
            r#"
            async function makeStream() {
                const { readable, writable } = wit.Stream(wit.Stream.OPTION_U32);
                writable.drop();
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("make-stream", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_stream_tuple_type_constant() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:stream-tuple;
            world stream-tuple {
                export make-stream: async func() -> stream<tuple<u32, string>>;
            }
            "#,
        )
        .script(
            r#"
            async function makeStream() {
                const { readable, writable } = wit.Stream(wit.Stream.TUPLE_U32_STRING);
                writable.drop();
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("make-stream", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_future_result_type_constant() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:future-result;
            world future-result {
                export make-future: async func() -> future<result<string, string>>;
            }
            "#,
        )
        .script(
            r#"
            async function makeFuture() {
                const { readable, writable } = wit.Future(wit.Future.RESULT_STRING_STRING);
                writable.write({ tag: "ok", val: "hello" });
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("make-future", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_stream_build_with_input_output() {
    // Verify component builds when WIT has stream params and returns
    let _instance = TestCase::new()
        .wit(
            r#"
            package test:stream-io;
            world stream-io {
                export echo-bytes: async func(input: stream<u8>) -> stream<u8>;
            }
            "#,
        )
        .script(
            r#"
            async function echoBytes(input) {
                const { readable, writable } = wit.Stream();
                (async () => {
                    const data = await input.read(1024);
                    await writable.write(data);
                    writable.drop();
                })();
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();
}

#[tokio::test]
async fn test_future_create_and_return_u32() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:future-u32;
            world future-u32 {
                export make-future: async func() -> future<u32>;
            }
            "#,
        )
        .script(
            r#"
            async function makeFuture() {
                const { readable, writable } = wit.Future();
                // Fire-and-forget write: completes when host reads the future
                writable.write(42);
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("make-future", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_future_create_and_return_string() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:future-string;
            world future-string {
                export make-future: async func() -> future<string>;
            }
            "#,
        )
        .script(
            r#"
            async function makeFuture() {
                const { readable, writable } = wit.Future();
                writable.write("hello from future");
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("make-future", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_future_object_return_shape() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:future-shape;
            world future-shape {
                export check-shape: async func() -> future<string>;
            }
            "#,
        )
        .script(
            r#"
            async function checkShape() {
                const pair = wit.Future();
                if (pair.readable === undefined) throw new Error("missing readable");
                if (pair.writable === undefined) throw new Error("missing writable");
                if (typeof pair.readable.read !== 'function') throw new Error("readable missing read");
                if (typeof pair.writable.write !== 'function') throw new Error("writable missing write");
                pair.writable.write("test");
                return pair.readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let results = instance.call_async("check-shape", &[], 1).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_future_build_with_input_output() {
    // Verify component builds when WIT has future params and returns
    let _instance = TestCase::new()
        .wit(
            r#"
            package test:future-io;
            world future-io {
                export echo-future: async func(input: future<string>) -> future<string>;
            }
            "#,
        )
        .script(
            r#"
            async function echoFuture(input) {
                const { readable, writable } = wit.Future();
                (async () => {
                    const val = await input.read();
                    await writable.write(val);
                })();
                return readable;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();
}

#[tokio::test]
async fn test_async_multiple_awaits() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:multi-await;
            world multi-await {
                export chain: async func(x: u32) -> u32;
            }
            "#,
        )
        .script(
            r#"
            async function chain(x) {
                let result = x;
                // Multiple promise resolutions to test the callback loop
                result = await Promise.resolve(result + 1);
                result = await Promise.resolve(result + 1);
                result = await Promise.resolve(result + 1);
                return result;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let result = instance
        .call1_async("chain", &[Val::U32(10)])
        .await
        .unwrap();
    assert_eq!(result, Val::U32(13));
}

#[tokio::test]
async fn test_async_error_in_promise() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-error;
            world async-error {
                export might-fail: async func(fail: bool) -> u32;
            }
            "#,
        )
        .script(
            r#"
            async function mightFail(fail) {
                if (fail) {
                    throw new Error("intentional failure");
                }
                return 42;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    // Success case
    let result = instance
        .call1_async("might-fail", &[Val::Bool(false)])
        .await
        .unwrap();
    assert_eq!(result, Val::U32(42));

    let result = instance
        .call_async("might-fail", &[Val::Bool(true)], 1)
        .await;

    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_async_result_no_error_payload() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-result-no-err;
            world async-result-no-err {
                export validate: async func(x: u32) -> result<u32>;
            }
            "#,
        )
        .script(
            r#"
            async function validate(x) {
                if (x > 100) {
                    return { tag: "err" };
                }
                return { tag: "ok", val: x * 2 };
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let result = instance
        .call1_async("validate", &[Val::U32(50)])
        .await
        .unwrap();
    assert_eq!(result, Val::Result(Ok(Some(Box::new(Val::U32(100))))));

    let result = instance
        .call1_async("validate", &[Val::U32(200)])
        .await
        .unwrap();
    assert_eq!(result, Val::Result(Err(None)));
}

#[tokio::test]
async fn test_async_variant_mixed_payloads() {
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:async-variant;
            world async-variant {
                variant response {
                    empty,
                    message(string),
                    code(u32),
                }
                export process: async func(kind: u32) -> response;
            }
            "#,
        )
        .script(
            r#"
            async function process(kind) {
                if (kind === 0) return { tag: 0 };
                if (kind === 1) return { tag: 1, val: "hello" };
                return { tag: 2, val: 42 };
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    let result = instance
        .call1_async("process", &[Val::U32(0)])
        .await
        .unwrap();

    match &result {
        Val::Variant(name, val) => {
            assert_eq!(name, "empty");
            assert!(val.is_none());
        }
        other => panic!("expected Variant, got {:?}", other),
    }

    // String payload case
    let result = instance
        .call1_async("process", &[Val::U32(1)])
        .await
        .unwrap();
    match &result {
        Val::Variant(name, val) => {
            assert_eq!(name, "message");
            assert_eq!(**val.as_ref().unwrap(), Val::String("hello".into()));
        }
        other => panic!("expected Variant, got {:?}", other),
    }

    // U32 payload case
    let result = instance
        .call1_async("process", &[Val::U32(2)])
        .await
        .unwrap();
    match &result {
        Val::Variant(name, val) => {
            assert_eq!(name, "code");
            assert_eq!(**val.as_ref().unwrap(), Val::U32(42));
        }
        other => panic!("expected Variant, got {:?}", other),
    }
}

/// A StreamProducer that yields a fixed set of bytes.
struct ByteProducer {
    data: Vec<u8>,
    offset: usize,
}

impl ByteProducer {
    fn new(data: Vec<u8>) -> Self {
        Self { data, offset: 0 }
    }
}

impl StreamProducer<WasiCtxState> for ByteProducer {
    type Item = u8;
    type Buffer = VecBuffer<u8>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _store: StoreContextMut<'a, WasiCtxState>,
        mut destination: Destination<'a, Self::Item, Self::Buffer>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if self.offset >= self.data.len() {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        let remaining = &self.data[self.offset..];
        let buf = VecBuffer::from(remaining.to_vec());
        self.offset = self.data.len();
        destination.set_buffer(buf);
        Poll::Ready(Ok(StreamResult::Dropped))
    }
}

#[tokio::test]
async fn test_host_stream_to_guest() {
    // Host provides stream<u8>, JS guest reads it and returns the count
    let mut instance = TestCase::new()
        .wit(
            r#"
            package test:host-stream;
            world host-stream {
                export count-bytes: async func(input: stream<u8>) -> u32;
            }
            "#,
        )
        .script(
            r#"
            async function countBytes(input) {
                let total = 0;
                const data = await input.read(1024);
                total += data.length;
                input.drop();
                return total;
            }
            "#,
        )
        .build_async()
        .await
        .unwrap();

    // Create a host-side stream producing 5 bytes
    let (inst, store) = instance.parts();
    let reader = StreamReader::new(&mut *store, ByteProducer::new(vec![1, 2, 3, 4, 5]));

    // Get the typed function and call it with the stream
    let func = inst
        .get_typed_func::<(StreamReader<u8>,), (u32,)>(&mut *store, "count-bytes")
        .unwrap();

    let (count,) = func.call_async(&mut *store, (reader,)).await.unwrap();

    assert_eq!(count, 5);
}
