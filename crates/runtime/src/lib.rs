mod abi;
mod bindings;
mod buffer;
mod call;
mod futures;
mod interpreter;
mod resources;
mod streams;
mod task;
mod trivia;

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use rquickjs::runtime::UserDataGuard;
use rquickjs::{Context, JsLifetime, Persistent, Runtime, Value, function};
use smallvec::SmallVec;
use task::TaskState;
use wit_dylib_ffi::Wit;

use crate::interpreter::WitData;
use crate::resources::BorrowedResource;
use crate::resources::ResourceTable;
use crate::trivia::*;

// Generate bindings for the init interface for wizer
mod init {
    wit_bindgen::generate!({
        world: "init",
        path: "wit/init.wit",
        generate_all,
        disable_run_ctors_once_workaround: true,
    });

    use super::InitImpl;
    export!(InitImpl);
}

// Global JS runtime and context.
static JS_STATE: SyncWrap<OnceCell<JsState>> = SyncWrap(OnceCell::new());

/// Wrapper to mark types as Sync for single-threaded WASM.
struct SyncWrap<T>(pub(crate) T);

// SAFETY: WASM execution is single-threaded for now.
unsafe impl<T> Sync for SyncWrap<T> {}

/// Global state for the quickjs runtime and context.
struct JsState {
    context: Context,
    /// Ensures the JavaScript source is only evaluated once during initialization.
    evaluated: AtomicBool,
    /// Cached active context pointer for re-entrant `with_ctx` calls.
    ctx_ptr: Cell<Option<*const ()>>,
}

/// Extension trait for `rquickjs::Ctx` providing convenient access to
/// runtime userdata.
pub(crate) trait CtxExt<'js> {
    /// Retrieve the WIT definition stored during initialization.
    fn wit(&self) -> Wit;

    /// Retrieve the async task state.
    fn task(&self) -> UserDataGuard<'_, TaskState>;

    /// Retrieve the exported resource table.
    fn resources(&self) -> UserDataGuard<'_, ResourceTable>;

    /// Retrieve the function/interface name cache.
    fn fns(&self) -> UserDataGuard<'_, FnNameCache>;
}

impl<'js> CtxExt<'js> for rquickjs::Ctx<'js> {
    fn wit(&self) -> Wit {
        self.userdata::<WitData>().expect("WIT not initialized").0
    }

    fn task(&self) -> UserDataGuard<'_, TaskState> {
        self.userdata().expect("TaskState not initialized")
    }

    fn resources(&self) -> UserDataGuard<'_, ResourceTable> {
        self.userdata().expect("ResourceTable not initialized")
    }

    fn fns(&self) -> UserDataGuard<'_, FnNameCache> {
        self.userdata().expect("FnNameCache not stored")
    }
}

impl JsState {
    fn get_or_init() -> &'static Self {
        JS_STATE.0.get_or_init(|| {
            let runtime = Runtime::new().expect("Failed to create quikcjs runtime");
            let context = Context::full(&runtime).expect("Failed to create quickjs context");

            context.with(|ctx| {
                ctx.store_userdata(FnNameCache::default())
                    .expect("Failed to store userdata");
            });

            JsState {
                context,
                evaluated: Default::default(),
                ctx_ptr: Default::default(),
            }
        })
    }

    /// Re-uses the active context if already inside `Context::with()` to avoid deadlock.
    ///
    /// This is needed for re-entrant flows such as export → host import callback → JS conversions.
    fn with_ctx<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&rquickjs::Ctx<'_>) -> R,
        R: 'static,
    {
        if let Some(ptr) = self.ctx_ptr.get() {
            // SAFETY: ptr is only set while a `Context::with()` frame is active.
            let ctx = unsafe { &*(ptr as *const rquickjs::Ctx<'_>) };
            return f(ctx);
        }

        self.context.with(|ctx| {
            let prev = self.ctx_ptr.replace(Some(std::ptr::from_ref(&ctx).cast()));
            let result = f(&ctx);
            self.ctx_ptr.set(prev);
            result
        })
    }
}

// Implements the init interface for wit-bindgen
struct InitImpl;

impl init::Guest for InitImpl {
    fn init(shim: String, js: String) -> Result<(), String> {
        init_js(&shim, &js)
    }
}

/// Call context for export/import invocations.
#[derive(Default)]
pub struct QjsCallContext {
    /// Value stack for WIT to JS: arguments in, results out
    stack: Vec<Persistent<Value<'static>>>,
    /// Tracks current index per nested list iteration
    iter_stack: SmallVec<[usize; 4]>,
    /// Keeps borrowed `&str` returns alive across FFI boundaries
    temp_strings: SmallVec<[String; 4]>,
    /// Raw allocations to free when this context is dropped
    deferred_deallocs: SmallVec<[(*mut u8, std::alloc::Layout); 4]>,
    /// Imported resource borrows to drop when this context is dropped
    borrows: SmallVec<[BorrowedResource; 4]>,
}

impl QjsCallContext {
    pub(crate) fn push_value<'js>(&mut self, ctx: &rquickjs::Ctx<'js>, val: Value<'js>) {
        self.stack.push(Persistent::save(ctx, val));
    }

    pub(crate) fn pop_value<'js>(&mut self, ctx: &rquickjs::Ctx<'js>) -> Value<'js> {
        self.pop_persistent().restore(ctx).expect("stack underflow")
    }

    pub(crate) fn pop_persistent(&mut self) -> Persistent<Value<'static>> {
        self.stack.pop().expect("stack underflow")
    }

    pub(crate) fn maybe_pop_persistent(&mut self) -> Option<Persistent<Value<'static>>> {
        self.stack.pop()
    }

    pub(crate) fn stack_into_args<'js>(&mut self, ctx: &rquickjs::Ctx<'js>) -> function::Args<'js> {
        let mut args = function::Args::new(ctx.clone(), self.stack.len());
        for p in self.stack.drain(..) {
            p.restore(ctx)
                .and_then(|val| args.push_arg(val))
                .expect("Failed to restore arg");
        }
        args
    }
}

impl Drop for QjsCallContext {
    fn drop(&mut self) {
        for (ptr, layout) in self.deferred_deallocs.drain(..) {
            unsafe {
                std::alloc::dealloc(ptr, layout);
            }
        }
        for borrow in self.borrows.drain(..) {
            unsafe {
                (borrow.drop_fn)(borrow.handle);
            }
        }
    }
}

/// Cache for converting WIT function/interface names to camelCase, stored as
/// rquickjs userdata so it is tied to the JS runtime lifetime.
#[derive(Default, JsLifetime)]
pub(crate) struct FnNameCache(RefCell<HashMap<&'static str, &'static str>>);

/// Initialize the quickjs runtime with JavaScript source code.
/// This is called by Wizer during pre-initialization.
fn init_js(shim: &str, js_source: &str) -> Result<(), String> {
    let state = JsState::get_or_init();

    if state.evaluated.swap(true, Ordering::SeqCst) {
        return Err("JavaScript already evaluated".to_string());
    }

    state.with_ctx(|ctx| {
        // Evaluate the generated shim first
        ctx.eval::<(), _>(shim)
            .map_err(|e| format!("Failed to evaluate shim: {:?}", e))?;
        // Then evaluate the user's script
        ctx.eval::<(), _>(js_source)
            .map_err(|e| format!("Failed to evaluate JavaScript: {:?}", e))
    })?;

    unsafe {
        abi::reset_adapter_state();
        abi::__wasilibc_reset_preopens();
    }

    Ok(())
}

/// Delegates to `JsState::with_ctx`.
pub(crate) fn with_ctx<F, R>(f: F) -> R
where
    F: FnOnce(&rquickjs::Ctx<'_>) -> R,
    R: 'static,
{
    JsState::get_or_init().with_ctx(f)
}
