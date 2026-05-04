//! `Interpreter` trait implementation for quickjs.
use crate::CtxExt;
use crate::abi::{CallbackCode, Event};
use crate::bindings::register;
use crate::resources::ResourceTable;
use crate::task::TaskState;
use crate::trivia::{fn_lookup, iface_lookup};
use crate::{QjsCallContext, abi, with_ctx};
use crate::{futures, streams};

use heck::ToUpperCamelCase;
use rquickjs::function::Constructor;
use rquickjs::{JsLifetime, Value};
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
            register(ctx, wit).expect("Failed to register WIT bindings");
        });
    }

    fn export_start<'a>(_wit: Wit, _func: ExportFunction) -> Box<Self::CallCx<'a>> {
        Box::new(QjsCallContext::default())
    }

    fn export_call(_wit: Wit, func: ExportFunction, cx: &mut Self::CallCx<'_>) {
        let name = func.name();

        if let Some(resource_name) = name.strip_prefix("[constructor]") {
            // Resource constructor: call `new ClassName(args...)` and store in table
            let class_name = resource_name.to_upper_camel_case();
            with_ctx(|ctx| {
                let exports = ctx
                    .user_module()
                    .exports(ctx)
                    .expect("user module exports not found");
                let ctor: Constructor = if let Some(iface) = func.interface() {
                    let iface_obj: rquickjs::Object = exports
                        .get(iface_lookup(ctx, iface))
                        .unwrap_or_else(|e| panic!("interface '{}' not found: {:?}", iface, e));
                    iface_obj
                        .get(class_name.as_str())
                        .unwrap_or_else(|e| panic!("class '{}' not found: {:?}", class_name, e))
                } else {
                    exports
                        .get(class_name.as_str())
                        .unwrap_or_else(|e| panic!("class '{}' not found: {:?}", class_name, e))
                };
                let args = cx.stack_into_args(ctx);
                let instance: Value = ctor
                    .construct_args(args)
                    .unwrap_or_else(|e| panic!("Failed to construct '{}': {:?}", class_name, e));

                cx.push_value(ctx, instance);
            });
        } else if let Some(rest) = name.strip_prefix("[method]") {
            // Resource method: first arg is `self` (resource handle), call method on it
            let (_resource, method_name) = rest
                .split_once('.')
                .unwrap_or_else(|| panic!("invalid method name: {name}"));

            with_ctx(|ctx| {
                let method_name = fn_lookup(ctx, method_name);
                // First param is the resource (self)
                let self_val = cx.pop_value(ctx);
                let self_obj = self_val
                    .as_object()
                    .unwrap_or_else(|| panic!("method receiver is not an object"));

                let method: rquickjs::Function = self_obj
                    .get(method_name)
                    .unwrap_or_else(|e| panic!("method '{}' not found: {:?}", method_name, e));

                let mut args = cx.stack_into_args(ctx);
                args.this(self_val).expect("failed to set this");

                let result = method
                    .call_arg::<Value>(args)
                    .unwrap_or_else(|e| panic!("Failed to call '{}': {:?}", method_name, e));

                if func.result().is_some() {
                    cx.push_value(ctx, result);
                }
            });
        } else if let Some(rest) = name.strip_prefix("[static]") {
            // Static resource method: look up Class on the interface object,
            // then call the static method on the class.
            let (resource, method_name) = rest
                .split_once('.')
                .unwrap_or_else(|| panic!("invalid static method name: {name}"));

            with_ctx(|ctx| {
                let method_name = fn_lookup(ctx, method_name);
                let class_name = resource.to_upper_camel_case();
                let exports = ctx
                    .user_module()
                    .exports(ctx)
                    .expect("user module exports not found");
                let class_obj: rquickjs::Object = if let Some(iface) = func.interface() {
                    let iface_obj: rquickjs::Object = exports
                        .get(iface_lookup(ctx, iface))
                        .unwrap_or_else(|e| panic!("interface '{}' not found: {:?}", iface, e));
                    iface_obj
                        .get(class_name.as_str())
                        .unwrap_or_else(|e| panic!("class '{}' not found: {:?}", class_name, e))
                } else {
                    exports
                        .get(class_name.as_str())
                        .unwrap_or_else(|e| panic!("class '{}' not found: {:?}", class_name, e))
                };
                let js_func: rquickjs::Function = class_obj.get(method_name).unwrap_or_else(|e| {
                    panic!("static method '{}' not found: {:?}", method_name, e)
                });
                let args = cx.stack_into_args(ctx);
                let result = js_func
                    .call_arg::<Value>(args)
                    .unwrap_or_else(|e| panic!("Failed to call '{}': {:?}", method_name, e));
                if func.result().is_some() {
                    cx.push_value(ctx, result);
                }
            });
        } else {
            // Regular function
            with_ctx(|ctx| {
                let exports = ctx
                    .user_module()
                    .exports(ctx)
                    .expect("user module exports not found");
                let func_name = fn_lookup(ctx, name);
                let js_func: rquickjs::Function = if let Some(iface) = func.interface() {
                    let iface_obj: rquickjs::Object = exports
                        .get(iface_lookup(ctx, iface))
                        .unwrap_or_else(|e| panic!("interface '{}' not found: {:?}", iface, e));
                    iface_obj
                        .get(func_name)
                        .unwrap_or_else(|e| panic!("function '{}' not found: {:?}", func_name, e))
                } else {
                    exports.get(func_name).unwrap_or_else(|e| {
                        panic!("Failed to get function '{}': {:?}", func_name, e)
                    })
                };
                let args = cx.stack_into_args(ctx);
                let result = js_func
                    .call_arg::<Value>(args)
                    .unwrap_or_else(|e| panic!("Failed to call '{}': {:?}", func.name(), e));

                if func.result().is_some() {
                    cx.push_value(ctx, result);
                }
            });
        }
    }

    fn export_async_start(
        _wit: Wit,
        func: ExportFunction,
        mut cx: Box<Self::CallCx<'static>>,
    ) -> u32 {
        with_ctx(|ctx| {
            ctx.task().init();

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

            let args = cx.stack_into_args(ctx);

            let _result = js_func
                .call_arg::<Value>(args)
                .unwrap_or_else(|e| panic!("Failed to call async '{}': {:?}", func.name(), e));
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
            Event::TaskCancelled => with_ctx(|ctx| ctx.task().cancel()),
        }

        if matches!(evt, Event::TaskCancelled) {
            CallbackCode::Exit.encode(0)
        } else {
            with_ctx(|ctx| ctx.task().poll())
        }
    }

    fn resource_dtor(_ty: Resource, handle: usize) {
        with_ctx(|ctx| {
            ctx.resources().remove(handle);
        });
    }
}

// Export FFI symbols
wit_dylib_ffi::export!(QjsInterpreter);
