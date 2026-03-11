//! Property-based echo tests using quickcheck.
//!
//! A single echo component is built once (cached) and reused across all test cases.
//! Each quickcheck test creates a fresh wasmtime instance from the cached wasm bytes.
mod common;

use std::fs;
use std::sync::{Mutex, OnceLock};

use quickcheck::{quickcheck, TestResult};
use tempfile::TempDir;
use wasmtime::component::Val;

use componentize_qjs::ComponentizeOpts;

use common::ComponentInstance;

/// JS Number.MAX_SAFE_INTEGER values beyond this lose precision in quickjs.
const MAX_SAFE_INT: i64 = (1i64 << 53) - 1;

/// Build the echo component once and cache the wasm bytes.
fn echo_wasm_bytes() -> &'static Vec<u8> {
    static ECHO_WASM: OnceLock<Vec<u8>> = OnceLock::new();
    ECHO_WASM.get_or_init(|| {
        let dir = TempDir::new().unwrap();
        let wit_path = dir.path().join("echo.wit");
        fs::write(
            &wit_path,
            r#"
            package test:echo;
            world echo {
                export echo-bool: func(v: bool) -> bool;
                export echo-u8: func(v: u8) -> u8;
                export echo-u16: func(v: u16) -> u16;
                export echo-u32: func(v: u32) -> u32;
                export echo-s32: func(v: s32) -> s32;
                export echo-u64: func(v: u64) -> u64;
                export echo-s64: func(v: s64) -> s64;
                export echo-f64: func(v: f64) -> f64;
                export echo-char: func(v: char) -> char;
                export echo-string: func(v: string) -> string;
            }
            "#,
        )
        .unwrap();

        let opts = ComponentizeOpts {
            wit_path: &wit_path,
            js_source: r#"
                function echoBool(v) { return v; }
                function echoU8(v) { return v; }
                function echoU16(v) { return v; }
                function echoU32(v) { return v; }
                function echoS32(v) { return v; }
                function echoU64(v) { return v; }
                function echoS64(v) { return v; }
                function echoF64(v) { return v; }
                function echoChar(v) { return v; }
                function echoString(v) { return v; }
            "#,
            world_name: None,
            stub_wasi: true,
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(componentize_qjs::componentize(&opts)).unwrap()
    })
}

fn echo_instance() -> &'static Mutex<ComponentInstance> {
    static INSTANCE: OnceLock<Mutex<ComponentInstance>> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let inst = ComponentInstance::from_wasm(echo_wasm_bytes().clone(), vec![], vec![]).unwrap();
        Mutex::new(inst)
    })
}

quickcheck! {
    fn qc_echo_bool(v: bool) -> bool {
        let mut inst = echo_instance().lock().unwrap();
        inst.call1("echo-bool", &[Val::Bool(v)]) == Val::Bool(v)
    }

    fn qc_echo_u8(v: u8) -> bool {
        let mut inst = echo_instance().lock().unwrap();
        inst.call1("echo-u8", &[Val::U8(v)]) == Val::U8(v)
    }

    fn qc_echo_u16(v: u16) -> bool {
        let mut inst = echo_instance().lock().unwrap();
        inst.call1("echo-u16", &[Val::U16(v)]) == Val::U16(v)
    }

    fn qc_echo_u32(v: u32) -> bool {
        let mut inst = echo_instance().lock().unwrap();
        inst.call1("echo-u32", &[Val::U32(v)]) == Val::U32(v)
    }

    fn qc_echo_s32(v: i32) -> bool {
        let mut inst = echo_instance().lock().unwrap();
        inst.call1("echo-s32", &[Val::S32(v)]) == Val::S32(v)
    }

    fn qc_echo_u64(v: u64) -> TestResult {
        if v > MAX_SAFE_INT as u64 {
            return TestResult::discard();
        }
        let mut inst = echo_instance().lock().unwrap();
        TestResult::from_bool(inst.call1("echo-u64", &[Val::U64(v)]) == Val::U64(v))
    }

    fn qc_echo_s64(v: i64) -> TestResult {
        if !(-MAX_SAFE_INT..=MAX_SAFE_INT).contains(&v) {
            return TestResult::discard();
        }
        let mut inst = echo_instance().lock().unwrap();
        TestResult::from_bool(inst.call1("echo-s64", &[Val::S64(v)]) == Val::S64(v))
    }

    fn qc_echo_f64(v: f64) -> bool {
        if !v.is_finite() {
            return true;
        }
        let mut inst = echo_instance().lock().unwrap();
        inst.call1("echo-f64", &[Val::Float64(v)]) == Val::Float64(v)
    }

    fn qc_echo_char(v: char) -> bool {
        let mut inst = echo_instance().lock().unwrap();
        inst.call1("echo-char", &[Val::Char(v)]) == Val::Char(v)
    }

    fn qc_echo_string(v: String) -> bool {
        let mut inst = echo_instance().lock().unwrap();
        inst.call1("echo-string", &[Val::String(v.clone())])
            == Val::String(v)
    }
}
