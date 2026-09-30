//! `Interpreter` trait implementation for quickjs.
use crate::CtxExt;
use crate::abi::Event;
use crate::bindings::register;
use crate::exports::prepare_export;
use crate::resources::{ResourceTable, drain_resource_drops};
use crate::result::ResultBoundary;
use crate::task::TaskState;
use crate::wit_imports::WitImportRegistry;
use crate::{QjsCallContext, with_ctx};
use crate::{abi, futures, streams};

use rquickjs::{CatchResultExt, JsLifetime};
use wit_dylib_ffi::{ExportFunction, Interpreter, Resource, Wit};

/// Newtype wrapper for `Wit` so it can be stored as rquickjs userdata.
#[derive(JsLifetime, Clone, Copy)]
pub(crate) struct WitData(pub(crate) Wit);

/// quickjs interpreter implementation of the `Interpreter` trait.
pub struct QjsInterpreter;

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
        with_ctx(|ctx| {
            let call = prepare_export(ctx, func, cx.drain_values())
                .catch(ctx)
                .unwrap_or_else(|err| panic!("Failed to resolve '{}': {err}", func.name()));

            let value = ResultBoundary::new(func.result())
                .lower_call(ctx, call.invoke())
                .unwrap_or_else(|err| panic!("Failed to call '{}': {err}", func.name()));

            if let Some(value) = value {
                cx.push_value(ctx, value);
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
            let values = cx.take_values();
            ctx.task().init(cx);

            prepare_export(ctx, func, values)
                .and_then(|call| call.invoke_async(ctx, func))
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
