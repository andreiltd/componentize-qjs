//! ES module loading and evaluated user module state.
mod host;
mod wit;

use std::cell::RefCell;

use rquickjs::{CaughtError, CaughtResult, Ctx, JsLifetime, Module, Persistent, Runtime};

use crate::CtxExt;

pub(crate) use wit::WitImportDeclarations;

pub(crate) fn install_loader(runtime: &Runtime) {
    runtime.set_loader(
        (wit::WitModuleResolver, host::HostModuleResolver),
        (wit::WitModuleLoader, host::HostModuleLoader),
    );
}

pub(crate) fn init_state(ctx: &rquickjs::Ctx<'_>) {
    ctx.store_userdata(UserModule::default())
        .expect("Failed to store user module state");
    ctx.store_userdata(WitImportDeclarations::default())
        .expect("Failed to store WIT import declaration state");
}

/// Stores the evaluated user ES module namespace as internal runtime state.
#[derive(Default)]
pub(crate) struct UserModule(RefCell<Option<Persistent<rquickjs::Object<'static>>>>);

// SAFETY: `UserModule` stores only a `Persistent<Object<'static>>`, which is
// tied to the owning QuickJS runtime and restored only for that same runtime.
unsafe impl<'js> JsLifetime<'js> for UserModule {
    type Changed<'to> = UserModule;
}

impl UserModule {
    fn store<'js>(&self, ctx: &rquickjs::Ctx<'js>, namespace: rquickjs::Object<'js>) {
        self.0.replace(Some(Persistent::save(ctx, namespace)));
    }

    pub(crate) fn exports<'js>(
        &self,
        ctx: &rquickjs::Ctx<'js>,
    ) -> rquickjs::Result<rquickjs::Object<'js>> {
        let namespace = self.0.borrow().as_ref().cloned().ok_or_else(|| {
            rquickjs::Error::new_from_js_message(
                "undefined",
                "module namespace",
                "user module was not evaluated",
            )
        })?;

        namespace.restore(ctx)
    }
}

pub(crate) fn evaluate_shim(ctx: &rquickjs::Ctx<'_>, shim: &str) -> Result<(), String> {
    evaluate(ctx, "componentize-qjs:shim.js", shim)
        .map(|_| ())
        .map_err(|e| format!("Failed to evaluate generated shim module: {e}"))
}

pub(crate) fn evaluate_user(
    ctx: &rquickjs::Ctx<'_>,
    js_source: &str,
    entry_path: Option<&str>,
) -> Result<(), String> {
    let namespace = evaluate(
        ctx,
        entry_path.unwrap_or("componentize-qjs:user.js"),
        js_source,
    )
    .map_err(|e| format!("Failed to evaluate user JavaScript module: {e}"))?;

    ctx.user_module().store(ctx, namespace);

    Ok(())
}

fn evaluate<'js>(
    ctx: &rquickjs::Ctx<'js>,
    name: &str,
    source: &str,
) -> Result<rquickjs::Object<'js>, String> {
    let module = CaughtError::catch(ctx, Module::declare(ctx.clone(), name, source))
        .map_err(|e| format!("Failed to declare JavaScript module: {e}"))?;
    let (module, promise) = CaughtError::catch(ctx, module.eval())
        .map_err(|e| format!("Failed to evaluate JavaScript module: {e}"))?;

    loop {
        if let Some(result) = promise.result::<()>() {
            CaughtError::catch(ctx, result)
                .map_err(|e| format!("Failed to finish JavaScript module evaluation: {e}"))?;
            break;
        }

        if !execute_pending_job(ctx).map_err(|e| format!("JavaScript module job failed: {e}"))? {
            return Err(format!(
                "Failed to finish JavaScript module evaluation: {}",
                rquickjs::Error::WouldBlock
            ));
        }
    }

    CaughtError::catch(ctx, module.namespace())
        .map_err(|e| format!("Failed to read JavaScript module namespace: {e}"))
}

/// Execute a job without reacquiring the runtime lock or losing its exception.
pub(crate) fn execute_pending_job<'js>(ctx: &Ctx<'js>) -> CaughtResult<'js, bool> {
    let mut job_ctx = std::ptr::null_mut();

    // The caller holds the runtime lock through Context::with. QuickJS returns
    // the context that owns an exception, which need not be the caller's context.
    let result = unsafe {
        let runtime = rquickjs::qjs::JS_GetRuntime(ctx.as_raw().as_ptr());
        rquickjs::qjs::JS_ExecutePendingJob(runtime, &mut job_ctx)
    };

    if result < 0 {
        let ptr = std::ptr::NonNull::new(job_ctx).expect("job exception without a context");
        // SAFETY: this context belongs to the locked runtime and cannot escape 'js.
        let job_ctx = unsafe { Ctx::from_raw(ptr) };

        Err(CaughtError::from_error(
            &job_ctx,
            rquickjs::Error::Exception,
        ))
    } else {
        Ok(result > 0)
    }
}
