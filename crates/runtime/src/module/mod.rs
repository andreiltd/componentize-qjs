//! ES module loading and evaluated user module state.
mod wit;

use std::cell::RefCell;

use rquickjs::{JsLifetime, Module, Persistent, Runtime};

use crate::CtxExt;

pub(crate) use wit::WitImportDeclarations;

pub(crate) fn install_loader(runtime: &Runtime) {
    runtime.set_loader(wit::WitModuleResolver, wit::WitModuleLoader);
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

pub(crate) fn evaluate_user(ctx: &rquickjs::Ctx<'_>, js_source: &str) -> Result<(), String> {
    let namespace = evaluate(ctx, "componentize-qjs:user.js", js_source)
        .map_err(|e| format!("Failed to evaluate user JavaScript module: {e}"))?;

    ctx.user_module().store(ctx, namespace);

    Ok(())
}

fn evaluate<'js>(
    ctx: &rquickjs::Ctx<'js>,
    name: &str,
    source: &str,
) -> Result<rquickjs::Object<'js>, String> {
    let (module, promise) = Module::declare(ctx.clone(), name, source)
        .map_err(|e| format!("Failed to declare JavaScript module: {:?}", e))?
        .eval()
        .map_err(|e| format!("Failed to evaluate JavaScript module: {:?}", e))?;

    promise
        .finish::<()>()
        .map_err(|e| format!("Failed to finish JavaScript module evaluation: {:?}", e))?;

    module
        .namespace()
        .map_err(|e| format!("Failed to read JavaScript module namespace: {:?}", e))
}
