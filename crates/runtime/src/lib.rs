mod abi;
mod bindings;
mod buffer;
mod call;
mod endpoint;
mod exports;
mod futures;
mod imports;
mod interpreter;
mod module;
mod resources;
mod result;
mod streams;
mod tagged;
mod task;
mod trivia;
mod typed_array;
mod wit_imports;

pub use call::QjsCallContext;

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::hash_map::DefaultHasher;

use rquickjs::runtime::UserDataGuard;
use rquickjs::{Context, JsLifetime, Runtime};
use task::TaskState;
use wit_dylib_ffi::Wit;

use crate::interpreter::WitData;
use crate::resources::{ResourceClasses, ResourceTable};
use crate::trivia::*;

/// Deterministic, fixed-seed hash map/set used everywhere in the runtime so the
/// wizer snapshot is reproducible.
pub(crate) type DetHasher = core::hash::BuildHasherDefault<DefaultHasher>;
pub(crate) type DetHashMap<K, V> = std::collections::HashMap<K, V, DetHasher>;
pub(crate) type DetHashSet<T> = std::collections::HashSet<T, DetHasher>;
pub(crate) type DetIndexMap<K, V> = indexmap::IndexMap<K, V, DetHasher>;

// Generate bindings for the init interface for wizer
#[allow(clippy::too_many_arguments)]
mod init {
    wit_bindgen::generate!({
        world: "init",
        path: "../core/wit/init.wit",
        generate_all,
        disable_run_ctors_once_workaround: true,
    });

    use super::InitImpl;
    export!(InitImpl);
}

// Version 1: NUL-terminated exports whose bodies are only needed before Wizer
// snapshots this runtime, including the linker-generated relocation routine.
#[cfg(target_family = "wasm")]
#[used]
#[unsafe(link_section = "componentize-qjs:init-only")]
static INIT_ONLY_EXPORTS: [u8; 67] =
    *b"\x01init\0cabi_post_init\0wit_dylib_initialize\0__wasm_apply_data_relocs\0";

// Global JS runtime and context.
static JS_STATE: GlobalJsState = GlobalJsState(OnceCell::new());

/// Global storage for the single-threaded Wasm runtime.
struct GlobalJsState(OnceCell<JsState>);

// SAFETY: WASM execution is single-threaded for now.
unsafe impl Sync for GlobalJsState {}

/// Global state for the quickjs runtime and context.
struct JsState {
    context: Context,
    /// Ensures the JavaScript source is only evaluated once during initialization.
    evaluated: Cell<bool>,
    /// Cached active context pointer for re-entrant `with_ctx` calls.
    ctx_ptr: Cell<Option<*const ()>>,
}

struct ActiveContextGuard<'a> {
    slot: &'a Cell<Option<*const ()>>,
    previous: Option<*const ()>,
}

impl<'a> ActiveContextGuard<'a> {
    fn enter(slot: &'a Cell<Option<*const ()>>, ctx: &rquickjs::Ctx<'_>) -> Self {
        let previous = slot.replace(Some(std::ptr::from_ref(ctx).cast()));
        Self { slot, previous }
    }
}

impl Drop for ActiveContextGuard<'_> {
    fn drop(&mut self) {
        self.slot.set(self.previous);
    }
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

    /// Retrieve the imported resource class/prototype registry.
    fn resource_classes(&self) -> UserDataGuard<'_, ResourceClasses>;

    /// Retrieve the function/interface name cache.
    fn fns(&self) -> UserDataGuard<'_, FnNameCache>;

    /// Retrieve the evaluated user ES module state.
    fn user_module(&self) -> UserDataGuard<'_, module::UserModule>;

    /// Retrieve transient WIT import module declaration state.
    fn wit_import_declarations(&self) -> UserDataGuard<'_, module::WitImportDeclarations>;

    /// Retrieve precomputed WIT import metadata.
    fn wit_import_registry(&self) -> UserDataGuard<'_, wit_imports::WitImportRegistry>;
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

    fn resource_classes(&self) -> UserDataGuard<'_, ResourceClasses> {
        self.userdata().expect("ResourceClasses not initialized")
    }

    fn fns(&self) -> UserDataGuard<'_, FnNameCache> {
        self.userdata().expect("FnNameCache not stored")
    }

    fn user_module(&self) -> UserDataGuard<'_, module::UserModule> {
        self.userdata().expect("UserModule not stored")
    }

    fn wit_import_declarations(&self) -> UserDataGuard<'_, module::WitImportDeclarations> {
        self.userdata().expect("WitImportDeclarations not stored")
    }

    fn wit_import_registry(&self) -> UserDataGuard<'_, wit_imports::WitImportRegistry> {
        self.userdata().expect("WitImportRegistry not stored")
    }
}

impl JsState {
    fn get_or_init() -> &'static Self {
        JS_STATE.0.get_or_init(|| {
            let runtime = Runtime::new().expect("Failed to create quikcjs runtime");
            module::install_loader(&runtime);
            let context = Context::full(&runtime).expect("Failed to create quickjs context");

            context.with(|ctx| {
                ctx.store_userdata(FnNameCache::default())
                    .expect("Failed to store function name cache");
                // Startup cleanup also runs for empty worlds without WIT initialization.
                ctx.store_userdata(ResourceClasses::default())
                    .expect("Failed to store ResourceClasses userdata");
                module::init_state(&ctx);
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
            let _guard = ActiveContextGuard::enter(&self.ctx_ptr, &ctx);
            f(&ctx)
        })
    }
}

// Implements the init interface for wit-bindgen
struct InitImpl;

impl init::Guest for InitImpl {
    fn init(
        shim: String,
        js: String,
        entry_path: Option<String>,
        disable_gc: bool,
    ) -> Result<(), String> {
        init_js(&shim, &js, entry_path.as_deref(), disable_gc)
    }
}

/// Cache for converting WIT function/interface names to camelCase, stored as
/// rquickjs userdata so it is tied to the JS runtime lifetime.
#[derive(Default, JsLifetime)]
pub(crate) struct FnNameCache(RefCell<DetHashMap<&'static str, &'static str>>);

/// Initialize the quickjs runtime with JavaScript source code.
/// This is called by Wizer during pre-initialization.
fn init_js(
    shim: &str,
    js_source: &str,
    entry_path: Option<&str>,
    disable_gc: bool,
) -> Result<(), String> {
    let state = JsState::get_or_init();

    if state.evaluated.replace(true) {
        return Err("JavaScript already evaluated".to_string());
    }

    if disable_gc {
        state.context.runtime().set_gc_threshold(usize::MAX);
    }

    state.with_ctx(|ctx| {
        module::evaluate_shim(ctx, shim)?;
        let result = module::evaluate_user(ctx, js_source, entry_path);
        resources::drain_resource_drops(ctx);
        result
    })?;

    unsafe {
        abi::reset_adapter_state();
        abi::__wasilibc_reset_preopens();
    }

    Ok(())
}

/// Access the initialized context without retaining its constructor after Wizer.
pub(crate) fn with_ctx<F, R>(f: F) -> R
where
    F: FnOnce(&rquickjs::Ctx<'_>) -> R,
    R: 'static,
{
    JS_STATE
        .0
        .get()
        .expect("JS runtime not initialized")
        .with_ctx(f)
}
