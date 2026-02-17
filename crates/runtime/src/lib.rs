//! QuickJS runtime implementing the wit-dylib-ffi Interpreter trait.
#![allow(unsafe_code)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![no_std]

extern crate alloc;

use core::cell::{Cell, OnceCell};
use core::sync::atomic::{AtomicBool, Ordering};

use hashbrown::HashMap;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use heck::{ToLowerCamelCase, ToUpperCamelCase};
use rquickjs::{function::Rest, Context, Persistent, Runtime, Value};
use wit_dylib_ffi::{
    Call, Enum, ExportFunction, Flags, Future, ImportFunction, Interpreter, List, Record, Resource,
    Stream, Tuple, Variant, Wit, WitOption, WitResult,
};

// Generate bindings for the init interface for wizer
mod bindings {
    wit_bindgen::generate!({
        world: "init",
        path: "wit/init.wit",
        generate_all,
        disable_run_ctors_once_workaround: true,
    });

    use super::InitImpl;
    export!(InitImpl);
}

#[link(wasm_import_module = "wasi_snapshot_preview1")]
unsafe extern "C" {
    #[link_name = "reset_adapter_state"]
    fn reset_adapter_state();
}

unsafe extern "C" {
    fn __wasilibc_reset_preopens();
}

/// Global JS state (Runtime + Context).
static JS_STATE: WasmSingleThreaded<OnceCell<JsState>> = WasmSingleThreaded(OnceCell::new());

/// Track whether JS has been evaluated
static JS_EVALUATED: AtomicBool = AtomicBool::new(false);

/// Cached context pointer for nested access.
static CACHED_CTX: WasmSingleThreaded<Cell<Option<*const ()>>> =
    WasmSingleThreaded(Cell::new(None));

struct JsState {
    _runtime: Runtime,
    context: Context,
}

/// Wrapper to mark types as Sync for single-threaded WASM.
struct WasmSingleThreaded<T>(T);

// SAFETY: WASM execution is single-threaded (for now).
unsafe impl<T> Sync for WasmSingleThreaded<T> {}

/// Initialize the QuickJS runtime with JavaScript source code.
/// This is called by Wizer during pre-initialization.
fn init_js(js_source: &str) -> Result<(), String> {
    if JS_EVALUATED.swap(true, Ordering::SeqCst) {
        return Err("JavaScript already evaluated".to_string());
    }

    with_ctx(|ctx| {
        ctx.eval::<(), _>(js_source)
            .map_err(|e| format!("Failed to evaluate JavaScript: {:?}", e))
    })?;

    unsafe {
        reset_adapter_state();
        __wasilibc_reset_preopens();
    }

    Ok(())
}

// Implement the init interface for wit-bindgen
struct InitImpl;

impl bindings::Guest for InitImpl {
    fn init(js: String) -> Result<(), String> {
        init_js(&js)
    }
}

/// Get the global JS context (shared reference).
fn js_context() -> &'static Context {
    &JS_STATE
        .0
        .get_or_init(|| {
            let runtime = Runtime::new().expect("Failed to create QuickJS runtime");
            let context = Context::full(&runtime).expect("Failed to create QuickJS context");
            JsState {
                _runtime: runtime,
                context,
            }
        })
        .context
}

/// Re-uses the active context if already inside `Context::with()` to avoid deadlock.
fn with_ctx<F, R>(f: F) -> R
where
    F: FnOnce(&rquickjs::Ctx<'_>) -> R,
    R: 'static,
{
    if let Some(ptr) = CACHED_CTX.0.get() {
        let ctx = unsafe { &*(ptr as *const rquickjs::Ctx<'_>) };
        f(ctx)
    } else {
        js_context().with(|ctx| {
            CACHED_CTX.0.set(Some(core::ptr::addr_of!(ctx) as *const ()));
            let result = f(&ctx);
            CACHED_CTX.0.set(None);
            result
        })
    }
}

// Global import lookups.
static WIT: WasmSingleThreaded<OnceCell<Wit>> = WasmSingleThreaded(OnceCell::new());

fn wit() -> Wit {
    *WIT.0.get().expect("WIT not initialized")
}

use core::alloc::Layout;

/// Call context for export/import invocations.
#[derive(Default)]
pub struct QjsCallContext {
    stack: Vec<Persistent<Value<'static>>>,
    temp_strings: Vec<String>,
    deferred_deallocs: Vec<(*mut u8, Layout)>,
}

impl Drop for QjsCallContext {
    fn drop(&mut self) {
        for (ptr, layout) in self.deferred_deallocs.drain(..) {
            unsafe {
                alloc::alloc::dealloc(ptr, layout);
            }
        }
    }
}

pub struct QjsInterpreter;

impl Interpreter for QjsInterpreter {
    type CallCx<'a> = QjsCallContext;

    fn initialize(wit_def: Wit) {
        WIT.0
            .set(wit_def)
            .unwrap_or_else(|_| panic!("WIT already initialized"));

        with_ctx(|ctx| {
            register_imports(ctx, wit_def).expect("Failed to register imports");
        });
    }

    fn export_start<'a>(_wit: Wit, _func: ExportFunction) -> Box<Self::CallCx<'a>> {
        Box::new(QjsCallContext::default())
    }

    fn export_call(_wit: Wit, func: ExportFunction, cx: &mut Self::CallCx<'_>) {
        with_ctx(|ctx| {
            let globals = ctx.globals();
            let func_name = func.name().to_lower_camel_case();
            let js_func: rquickjs::Function = globals
                .get(func_name.as_str())
                .unwrap_or_else(|e| panic!("Failed to get function '{}': {:?}", func_name, e));

            let arg_count = cx.stack.len();
            let mut args = rquickjs::function::Args::new(ctx.clone(), arg_count);
            for persistent in cx.stack.drain(..) {
                persistent
                    .restore(ctx)
                    .and_then(|val| args.push_arg(val))
                    .expect("Failed to restore argument");
            }

            let result = js_func
                .call_arg::<Value>(args)
                .unwrap_or_else(|e| panic!("Failed to call '{}': {:?}", func.name(), e));

            if func.result().is_some() {
                cx.stack.push(Persistent::save(ctx, result));
            }
        });
    }

    fn export_finish(_cx: Box<Self::CallCx<'_>>, _func: ExportFunction) {}
    fn resource_dtor(_ty: Resource, _handle: usize) {}
}

// Import Bindings
#[derive(Default)]
struct JsInterface {
    funcs: Vec<ImportFunction>,
    flags: Vec<Flags>,
    enums: Vec<Enum>,
}

fn partition_imports(wit: Wit) -> HashMap<Option<&'static str>, JsInterface> {
    let mut ret: HashMap<_, JsInterface> = HashMap::new();
    for func in wit.iter_import_funcs() {
        ret.entry(func.interface()).or_default().funcs.push(func);
    }

    for flags in wit.iter_flags() {
        if flags.interface().is_some() {
            ret.entry(flags.interface()).or_default().flags.push(flags);
        }
    }

    for enum_ty in wit.iter_enums() {
        if enum_ty.interface().is_some() {
            ret.entry(enum_ty.interface())
                .or_default()
                .enums
                .push(enum_ty);
        }
    }
    ret
}

fn register_imports(ctx: &rquickjs::Ctx<'_>, wit: Wit) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    let imports = partition_imports(wit);

    for (name, interface) in imports.iter() {
        let obj = create_interface_object(ctx, interface)?;
        match name {
            Some(name) => {
                let name_no_version = name.split('@').next().unwrap_or(name);
                globals.set(name_no_version, obj.clone())?;
                globals.set(*name, obj)?;
            }
            None => {
                for key in obj.keys::<String>() {
                    let key = key?;
                    let val: Value = obj.get(&key)?;
                    globals.set(key, val)?;
                }
            }
        }
    }
    Ok(())
}

fn create_interface_object<'js>(
    ctx: &rquickjs::Ctx<'js>,
    interface: &JsInterface,
) -> rquickjs::Result<rquickjs::Object<'js>> {
    let obj = rquickjs::Object::new(ctx.clone())?;

    for flags in &interface.flags {
        let flags_obj = rquickjs::Object::new(ctx.clone())?;
        for (i, name) in flags.names().enumerate() {
            flags_obj.set(name.to_upper_camel_case(), 1u32 << i)?;
        }
        obj.set(flags.name().to_upper_camel_case(), flags_obj)?;
    }

    for enum_ty in &interface.enums {
        let enum_obj = rquickjs::Object::new(ctx.clone())?;
        for (i, name) in enum_ty.names().enumerate() {
            let i = i as u32;
            enum_obj.set(name.to_upper_camel_case(), i)?;
            enum_obj.set(i, name)?;
        }
        obj.set(enum_ty.name().to_upper_camel_case(), enum_obj)?;
    }

    for func in &interface.funcs {
        let func_name = func.name().to_lower_camel_case();
        let func_index = func.index();
        let js_func = rquickjs::Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'js>, args: Rest<Value<'js>>| {
                call_import(ctx, func_index, args.0)
            },
        )?;
        obj.set(func_name, js_func)?;
    }

    Ok(obj)
}

fn call_import<'js>(
    ctx: rquickjs::Ctx<'js>,
    func_index: usize,
    args: Vec<Value<'js>>,
) -> rquickjs::Result<Value<'js>> {
    let wit_def = wit();
    let func = wit_def.import_func(func_index);

    let mut call = QjsCallContext::default();
    for arg in args.into_iter().rev() {
        call.stack.push(Persistent::save(&ctx, arg));
    }

    func.call_import_sync(&mut call);

    match call.stack.pop() {
        Some(persistent) => persistent.restore(&ctx),
        None => Ok(Value::new_undefined(ctx)),
    }
}

// Calls
impl Call for QjsCallContext {
    unsafe fn defer_deallocate(&mut self, ptr: *mut u8, layout: Layout) {
        self.deferred_deallocs.push((ptr, layout));
    }

    fn pop_bool(&mut self) -> bool {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.as_bool().expect("expected bool")
        })
    }

    fn pop_u8(&mut self) -> u8 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let n: i32 = val.get().expect("expected number");
            n as u8
        })
    }

    fn pop_s8(&mut self) -> i8 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let n: i32 = val.get().expect("expected number");
            n as i8
        })
    }

    fn pop_u16(&mut self) -> u16 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let n: i32 = val.get().expect("expected number");
            n as u16
        })
    }

    fn pop_s16(&mut self) -> i16 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let n: i32 = val.get().expect("expected number");
            n as i16
        })
    }

    fn pop_u32(&mut self) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get().expect("expected number")
        })
    }

    fn pop_s32(&mut self) -> i32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get().expect("expected number")
        })
    }

    fn pop_u64(&mut self) -> u64 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get().expect("expected number")
        })
    }

    fn pop_s64(&mut self) -> i64 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get().expect("expected number")
        })
    }

    fn pop_f32(&mut self) -> f32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let n: f64 = val.get().expect("expected number");
            n as f32
        })
    }

    fn pop_f64(&mut self) -> f64 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get().expect("expected number")
        })
    }

    fn pop_char(&mut self) -> char {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let s: String = val.get().expect("expected string");
            s.chars().next().expect("expected non-empty string")
        })
    }

    fn pop_string(&mut self) -> &str {
        let persistent = self.stack.pop().expect("stack underflow");
        let s = with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get::<String>().expect("expected string")
        });
        self.temp_strings.push(s);
        self.temp_strings.last().unwrap()
    }

    fn pop_list(&mut self, _ty: List) -> usize {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let arr = val.as_array().expect("expected array");
            let len = arr.len();
            for i in (0..len).rev() {
                let elem: Value = arr.get(i).unwrap();
                self.stack.push(Persistent::save(ctx, elem));
            }
            len
        })
    }

    fn pop_iter(&mut self, _ty: List) {}
    fn pop_iter_next(&mut self, _ty: List) {}

    fn pop_option(&mut self, _ty: WitOption) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            if val.is_null() || val.is_undefined() {
                0
            } else {
                self.stack.push(Persistent::save(ctx, val));
                1
            }
        })
    }

    fn pop_result(&mut self, _ty: WitResult) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let obj = val.as_object().expect("expected object");
            let tag: String = obj.get("tag").expect("expected tag");
            let inner: Value = obj.get("val").unwrap_or(Value::new_undefined(ctx.clone()));
            self.stack.push(Persistent::save(ctx, inner));
            if tag == "ok" {
                0
            } else {
                1
            }
        })
    }

    fn pop_variant(&mut self, _ty: Variant) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let obj = val.as_object().expect("expected object");
            let tag: u32 = obj.get("tag").expect("expected tag");
            let inner: Value = obj.get("val").unwrap_or(Value::new_undefined(ctx.clone()));
            self.stack.push(Persistent::save(ctx, inner));
            tag
        })
    }

    fn pop_enum(&mut self, _ty: Enum) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get().expect("expected number")
        })
    }

    fn pop_flags(&mut self, _ty: Flags) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get().expect("expected number")
        })
    }

    fn pop_borrow(&mut self, _ty: Resource) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get().expect("expected number")
        })
    }

    fn pop_own(&mut self, _ty: Resource) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get().expect("expected number")
        })
    }

    fn pop_tuple(&mut self, ty: Tuple) {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let arr = val.as_array().expect("expected array");
            for i in (0..ty.types().len()).rev() {
                let elem: Value = arr.get(i).unwrap();
                self.stack.push(Persistent::save(ctx, elem));
            }
        });
    }

    fn pop_record(&mut self, ty: Record) {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let obj = val.as_object().expect("expected object");
            for (name, _) in ty.fields().rev() {
                let field: Value = obj.get(name.to_lower_camel_case()).unwrap();
                self.stack.push(Persistent::save(ctx, field));
            }
        });
    }

    fn pop_future(&mut self, _ty: Future) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get().expect("expected number")
        })
    }

    fn pop_stream(&mut self, _ty: Stream) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get().expect("expected number")
        })
    }

    // Push operations
    fn push_bool(&mut self, val: bool) {
        with_ctx(|ctx| {
            let v = Value::new_bool(ctx.clone(), val);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_u8(&mut self, val: u8) {
        with_ctx(|ctx| {
            let v = Value::new_int(ctx.clone(), val as i32);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_s8(&mut self, val: i8) {
        with_ctx(|ctx| {
            let v = Value::new_int(ctx.clone(), val as i32);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_u16(&mut self, val: u16) {
        with_ctx(|ctx| {
            let v = Value::new_int(ctx.clone(), val as i32);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_s16(&mut self, val: i16) {
        with_ctx(|ctx| {
            let v = Value::new_int(ctx.clone(), val as i32);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_u32(&mut self, val: u32) {
        with_ctx(|ctx| {
            let v = Value::new_number(ctx.clone(), val as f64);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_s32(&mut self, val: i32) {
        with_ctx(|ctx| {
            let v = Value::new_int(ctx.clone(), val);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_u64(&mut self, val: u64) {
        with_ctx(|ctx| {
            let v = Value::new_number(ctx.clone(), val as f64);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_s64(&mut self, val: i64) {
        with_ctx(|ctx| {
            let v = Value::new_number(ctx.clone(), val as f64);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_f32(&mut self, val: f32) {
        with_ctx(|ctx| {
            let v = Value::new_number(ctx.clone(), val as f64);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_f64(&mut self, val: f64) {
        with_ctx(|ctx| {
            let v = Value::new_number(ctx.clone(), val);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_char(&mut self, val: char) {
        with_ctx(|ctx| {
            let s = val.to_string();
            let v = rquickjs::String::from_str(ctx.clone(), &s)
                .unwrap()
                .into_value();
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_string(&mut self, val: String) {
        with_ctx(|ctx| {
            let v = rquickjs::String::from_str(ctx.clone(), &val)
                .unwrap()
                .into_value();
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_list(&mut self, _ty: List, _capacity: usize) {
        with_ctx(|ctx| {
            let arr = rquickjs::Array::new(ctx.clone()).unwrap();
            self.stack.push(Persistent::save(ctx, arr.into_value()));
        });
    }

    fn list_append(&mut self, _ty: List) {
        let elem = self.stack.pop().expect("stack underflow");
        let arr_persistent = self.stack.last().expect("stack underflow").clone();
        with_ctx(|ctx| {
            let arr_val = arr_persistent.restore(ctx).unwrap();
            let arr = arr_val.as_array().expect("expected array");
            let val = elem.restore(ctx).unwrap();
            let len = arr.len();
            arr.set(len, val).unwrap();
        });
    }

    fn push_option(&mut self, _ty: WitOption, is_some: bool) {
        if !is_some {
            with_ctx(|ctx| {
                self.stack
                    .push(Persistent::save(ctx, Value::new_null(ctx.clone())));
            });
        }
    }

    fn push_result(&mut self, ty: WitResult, is_err: bool) {
        // Only pop if the ok/err case has a payload type
        let has_payload = if is_err {
            ty.err().is_some()
        } else {
            ty.ok().is_some()
        };
        let inner = if has_payload { self.stack.pop() } else { None };

        with_ctx(|ctx| {
            let obj = rquickjs::Object::new(ctx.clone()).unwrap();
            obj.set("tag", if is_err { "err" } else { "ok" }).unwrap();
            if let Some(val) = inner {
                let v = val.restore(ctx).unwrap();
                obj.set("val", v).unwrap();
            }
            self.stack.push(Persistent::save(ctx, obj.into_value()));
        });
    }

    fn push_variant(&mut self, ty: Variant, tag: u32) {
        // Only pop if this variant case has a payload type
        let has_payload = ty
            .cases()
            .nth(tag as usize)
            .map(|(_, ty)| ty.is_some())
            .unwrap_or(false);
        let inner = if has_payload { self.stack.pop() } else { None };

        with_ctx(|ctx| {
            let obj = rquickjs::Object::new(ctx.clone()).unwrap();
            obj.set("tag", tag).unwrap();
            if let Some(val) = inner {
                let v = val.restore(ctx).unwrap();
                obj.set("val", v).unwrap();
            }
            self.stack.push(Persistent::save(ctx, obj.into_value()));
        });
    }

    fn push_enum(&mut self, _ty: Enum, val: u32) {
        with_ctx(|ctx| {
            let v = Value::new_int(ctx.clone(), val as i32);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_flags(&mut self, _ty: Flags, val: u32) {
        with_ctx(|ctx| {
            let v = Value::new_number(ctx.clone(), val as f64);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_borrow(&mut self, _ty: Resource, handle: u32) {
        with_ctx(|ctx| {
            let v = Value::new_number(ctx.clone(), handle as f64);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_own(&mut self, _ty: Resource, handle: u32) {
        with_ctx(|ctx| {
            let v = Value::new_number(ctx.clone(), handle as f64);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_tuple(&mut self, ty: Tuple) {
        let len = ty.types().len();
        let mut elems = Vec::new();
        for _ in 0..len {
            elems.push(self.stack.pop().expect("stack underflow"));
        }
        with_ctx(|ctx| {
            let arr = rquickjs::Array::new(ctx.clone()).unwrap();
            for (i, elem) in elems.into_iter().rev().enumerate() {
                let val = elem.restore(ctx).unwrap();
                arr.set(i, val).unwrap();
            }
            self.stack.push(Persistent::save(ctx, arr.into_value()));
        });
    }

    fn push_record(&mut self, ty: Record) {
        let fields: Vec<_> = ty.fields().collect();
        let mut vals = Vec::new();
        for _ in &fields {
            vals.push(self.stack.pop().expect("stack underflow"));
        }
        with_ctx(|ctx| {
            let obj = rquickjs::Object::new(ctx.clone()).unwrap();
            for ((name, _), val) in fields.iter().zip(vals.into_iter().rev()) {
                let v = val.restore(ctx).unwrap();
                obj.set(name.to_lower_camel_case(), v).unwrap();
            }
            self.stack.push(Persistent::save(ctx, obj.into_value()));
        });
    }

    fn push_future(&mut self, _ty: Future, handle: u32) {
        with_ctx(|ctx| {
            let v = Value::new_int(ctx.clone(), handle as i32);
            self.stack.push(Persistent::save(ctx, v));
        });
    }

    fn push_stream(&mut self, _ty: Stream, handle: u32) {
        with_ctx(|ctx| {
            let v = Value::new_int(ctx.clone(), handle as i32);
            self.stack.push(Persistent::save(ctx, v));
        });
    }
}

// Export FFI symbols
wit_dylib_ffi::export!(QjsInterpreter);
