//! Verify resource lifetimes when a pending host import is cancelled.
#![cfg(feature = "component-model-async")]

mod common;

use common::{TestCase, WasiCtxState, async_engine, wasi_wit_dir};
use wasmtime::Store;
use wasmtime::component::{Linker, ResourceTable, Val};
use wasmtime_wasi::WasiCtxBuilder;

fn wasi_fixture(wit: &str) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("deps")).unwrap();
    for package in std::fs::read_dir(wasi_wit_dir().join("deps")).unwrap() {
        let package = package.unwrap();
        std::fs::copy(
            package.path().join("package.wit"),
            dir.path()
                .join("deps")
                .join(format!("{}.wit", package.file_name().to_string_lossy())),
        )
        .unwrap();
    }
    std::fs::write(dir.path().join("test.wit"), wit).unwrap();
    dir
}

#[tokio::test]
async fn cancelling_import_releases_borrowed_resources_after_settlement() {
    let dir = wasi_fixture(
        r#"
        package test:loans;
        interface service {
            use wasi:io/streams@0.2.12.{input-stream};
            compute: async func(input: borrow<input-stream>);
        }
        world loans {
            import wasi:cli/stdin@0.2.12;
            import service;
            export run: async func() -> bool;
        }
        "#,
    );
    let component = TestCase::new()
        .wit_dir(dir.path())
        .script(
            r#"
            import { getStdin } from "wasi:cli/stdin@0.2.12";
            import { compute } from "test:loans/service";
            export async function run() {
                const input = getStdin();
                const pending = compute(input);
                const rejected = pending.catch(error => error.message);
                let loanProtected = false;
                try { input[Symbol.dispose](); }
                catch { loanProtected = true; }
                pending.cancel();
                if (await rejected !== "async import cancelled") {
                    throw new Error("import did not reject");
                }
                // The host no longer borrows this handle once the promise settles.
                input[Symbol.dispose]();
                pending.cancel();
                return loanProtected;
            }
            "#,
        )
        .compile_async()
        .await
        .unwrap();

    let engine = async_engine();
    let mut linker = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker).unwrap();
    let mut service = linker.instance("test:loans/service").unwrap();
    service
        .resource(
            "input-stream",
            wasmtime::component::ResourceType::host::<wasmtime_wasi::p2::DynInputStream>(),
            |_, _| Ok(()),
        )
        .unwrap();
    service
        .func_wrap_concurrent(
            "compute",
            |_, (_input,): (wasmtime::component::Resource<wasmtime_wasi::p2::DynInputStream>,)| {
                Box::pin(std::future::pending::<wasmtime::Result<()>>())
            },
        )
        .unwrap();
    let mut store = Store::new(
        engine,
        WasiCtxState {
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
        },
    );
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    let run = instance.get_func(&mut store, "run").unwrap();
    for _ in 0..2 {
        let mut results = [Val::Bool(false)];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            run.call_async(&mut store, &[], &mut results),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(results, [Val::Bool(true)]);
        assert!(
            store.data().table.is_empty(),
            "cancelled call leaked resources"
        );
    }
}
