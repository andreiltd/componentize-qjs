//! `Interpreter` trait implementation for quickjs.
use crate::CtxExt;
use crate::abi::Event;
use crate::bindings::register;
use crate::resources::{ResourceTable, drain_resource_drops};
use crate::result::ResultBoundary;
use crate::task::TaskState;
use crate::trivia::{fn_lookup, iface_lookup};
use crate::wit_imports::{FuncKind, WitImportRegistry, classify};
use crate::{QjsCallContext, with_ctx};
use crate::{abi, futures, streams};

use heck::ToUpperCamelCase;
use rquickjs::function::{Args, Constructor};
use rquickjs::{CatchResultExt, Ctx, Function, JsLifetime, Object, Value};
use wit_dylib_ffi::{ExportFunction, Interpreter, Resource, Wit};

/// Newtype wrapper for `Wit` so it can be stored as rquickjs userdata.
#[derive(JsLifetime, Clone, Copy)]
pub(crate) struct WitData(pub(crate) Wit);

/// quickjs interpreter implementation of the `Interpreter` trait.
pub struct QjsInterpreter;

/// Select the root or interface-scoped namespace for an exported function.
fn export_scope<'js>(ctx: &Ctx<'js>, interface: Option<&'static str>) -> Object<'js> {
    let exports = ctx
        .user_module()
        .exports(ctx)
        .expect("user module exports not found");

    match interface {
        Some(interface) => exports
            .get(iface_lookup(ctx, interface))
            .unwrap_or_else(|err| panic!("interface '{interface}' not found: {err:?}")),
        None => exports,
    }
}

/// Call a JavaScript export and lower its return/throw through the WIT boundary.
fn call_export<'js>(
    ctx: &Ctx<'js>,
    func: ExportFunction,
    name: &str,
    js_func: Function<'js>,
    args: Args<'js>,
    cx: &mut QjsCallContext,
) {
    let value = ResultBoundary::new(func.result())
        .lower_call(ctx, js_func.call_arg::<Value>(args))
        .unwrap_or_else(|err| panic!("Failed to call '{name}': {err:?}"));

    if let Some(value) = value {
        cx.push_value(ctx, value);
    }
}

impl Interpreter for QjsInterpreter {
    type CallCx<'a> = QjsCallContext;

    fn initialize(wit: Wit) {
        with_ctx(|ctx| {
            ctx.store_userdata(WitData(wit))
                .expect("Failed to store WIT userdata");
            ctx.store_userdata(ResourceTable::default())
                .expect("Failed to store ResourceTable userdata");
            ctx.store_userdata(TaskState::new())
                .expect("Failed to store TaskState userdata");
            ctx.store_userdata(WitImportRegistry::new(wit))
                .expect("Failed to store WIT import registry");
            register(ctx, wit).expect("Failed to register WIT bindings");
        });
    }

    fn export_start<'a>(_wit: Wit, _func: ExportFunction) -> Box<Self::CallCx<'a>> {
        with_ctx(drain_resource_drops);
        Box::new(QjsCallContext::default())
    }

    fn export_call(_wit: Wit, func: ExportFunction, cx: &mut Self::CallCx<'_>) {
        with_ctx(|ctx| match classify(func.name()) {
            FuncKind::Constructor { resource } => {
                let class_name = resource.to_upper_camel_case();
                let scope = export_scope(ctx, func.interface());
                let ctor: Constructor = scope
                    .get(class_name.as_str())
                    .unwrap_or_else(|err| panic!("class '{class_name}' not found: {err:?}"));
                let args = cx.stack_into_args(ctx);
                let instance: Value = ctor
                    .construct_args(args)
                    .unwrap_or_else(|err| panic!("Failed to construct '{class_name}': {err:?}"));
                cx.push_value(ctx, instance);
            }
            FuncKind::Method { method, .. } => {
                assert!(!method.is_empty(), "invalid method name: {}", func.name());
                let method_name = fn_lookup(ctx, method);
                let self_val = cx.shift_value(ctx);
                let self_obj = self_val
                    .as_object()
                    .expect("method receiver is not an object");
                let method: Function = self_obj
                    .get(method_name)
                    .unwrap_or_else(|err| panic!("method '{method_name}' not found: {err:?}"));
                let mut args = cx.stack_into_args(ctx);
                args.this(self_val).expect("failed to set this");
                call_export(ctx, func, method_name, method, args, cx);
            }
            FuncKind::Static { resource, method } => {
                assert!(
                    !method.is_empty(),
                    "invalid static method name: {}",
                    func.name()
                );
                let method_name = fn_lookup(ctx, method);
                let class_name = resource.to_upper_camel_case();
                let scope = export_scope(ctx, func.interface());
                let class: Object = scope
                    .get(class_name.as_str())
                    .unwrap_or_else(|err| panic!("class '{class_name}' not found: {err:?}"));
                let js_func: Function = class.get(method_name).unwrap_or_else(|err| {
                    panic!("static method '{method_name}' not found: {err:?}")
                });
                let mut args = cx.stack_into_args(ctx);
                args.this(class).expect("failed to set static this");
                call_export(ctx, func, method_name, js_func, args, cx);
            }
            FuncKind::Freestanding => {
                let func_name = fn_lookup(ctx, func.name());
                let scope = export_scope(ctx, func.interface());
                let js_func: Function = scope
                    .get(func_name)
                    .unwrap_or_else(|err| panic!("function '{func_name}' not found: {err:?}"));
                let args = cx.stack_into_args(ctx);
                call_export(ctx, func, func_name, js_func, args, cx);
            }
        });
        with_ctx(drain_resource_drops);
    }

    fn export_async_start(
        _wit: Wit,
        func: ExportFunction,
        mut cx: Box<Self::CallCx<'static>>,
    ) -> u32 {
        with_ctx(|ctx| {
            drain_resource_drops(ctx);
            let args = cx.stack_into_args(ctx);
            ctx.task().init(cx);

            let globals = ctx.globals();

            let cqjs: rquickjs::Object = globals.get("__cqjs").expect("__cqjs namespace not found");

            let async_exports: rquickjs::Object = cqjs
                .get("asyncExports")
                .expect("__cqjs.asyncExports not found");

            let wrapper_obj = if let Some(interface) = func.interface() {
                async_exports.get(iface_lookup(ctx, interface)).unwrap()
            } else {
                async_exports
            };

            let func_name = fn_lookup(ctx, func.name());
            let js_func: rquickjs::Function = wrapper_obj
                .get(func_name)
                .unwrap_or_else(|e| panic!("Failed to get async export '{}': {:?}", func_name, e));

            let _result = js_func
                .call_arg::<Value>(args)
                .catch(ctx)
                .unwrap_or_else(|e| panic!("Failed to call async '{}': {e}", func.name()));
        });

        with_ctx(|ctx| ctx.task().poll())
    }

    fn export_async_callback(event0: u32, event1: u32, event2: u32) -> u32 {
        // Restore task state from host context
        with_ctx(|ctx| {
            let ptr = unsafe { abi::context_get() } as usize;
            ctx.task().restore(ptr);
            unsafe { abi::context_set(0) };
        });

        let evt = Event::decode(event0, event1, event2);

        match evt {
            Event::None => {}
            Event::Subtask { handle, state } => crate::task::handle_subtask(handle, state),
            Event::StreamWrite { handle, result } => streams::handle_write_event(handle, result),
            Event::StreamRead { handle, result } => streams::handle_read_event(handle, result),
            Event::FutureWrite { handle, result } => futures::handle_write_event(handle, result),
            Event::FutureRead { handle, result } => futures::handle_read_event(handle, result),
            Event::TaskCancelled => with_ctx(|ctx| {
                ctx.task()
                    .cancel(ctx)
                    .catch(ctx)
                    .expect("Failed to cancel async task");
            }),
        }

        with_ctx(|ctx| ctx.task().poll())
    }

    fn export_finish(mut cx: Box<Self::CallCx<'_>>, _func: ExportFunction) {
        // Post-return cannot invoke host destructors; queued drops wait for the next entry.
        cx.complete_transfers(1);
        drop(cx);
    }

    fn resource_dtor(_ty: Resource, handle: usize) {
        with_ctx(|ctx| {
            ctx.resources().remove(handle);
            drain_resource_drops(ctx);
        });
    }
}

// Export FFI symbols
wit_dylib_ffi::export!(QjsInterpreter);
