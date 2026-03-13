//! WIT to/from JS binding registration.
use std::collections::HashMap;

use heck::{ToLowerCamelCase, ToUpperCamelCase};
use rquickjs::function::{self, Rest};
use rquickjs::{Ctx, Function, Persistent, Value};
use wit_dylib_ffi::{Enum, Flags, ImportFunction, Variant, Wit};

use crate::CtxExt;
use crate::futures::{make_future, register_future_classes};
use crate::streams::{make_stream, register_stream_classes};
use crate::task::Pending;
use crate::{QjsCallContext, coerce_fn};

/// Register all wit bindings on the js global scope.
pub(crate) fn register(ctx: &rquickjs::Ctx<'_>, wit_def: Wit) -> rquickjs::Result<()> {
    register_stream_classes(ctx)?;
    register_future_classes(ctx)?;
    register_imports(ctx, wit_def)?;
    register_async_exports(ctx, wit_def)?;
    register_stream_future_factories(ctx)?;
    register_memory_introspection(ctx)?;
    Ok(())
}

/// Groups wit imports belonging to one interface or the root scope.
#[derive(Default)]
struct WitInterface {
    funcs: Vec<ImportFunction>,
    flags: Vec<Flags>,
    enums: Vec<Enum>,
    variants: Vec<Variant>,
}

/// Partition all wit imports by interface name.
fn partition_imports(wit: Wit) -> HashMap<Option<&'static str>, WitInterface> {
    let mut ret: HashMap<_, WitInterface> = HashMap::new();

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
    for variant in wit.iter_variants() {
        if variant.interface().is_some() {
            ret.entry(variant.interface())
                .or_default()
                .variants
                .push(variant);
        }
    }
    ret
}

/// Create a js object containing all functions, flags, enums, and variants
/// for a single wit interface.
fn interface_to_js<'js>(
    ctx: &rquickjs::Ctx<'js>,
    iface: &WitInterface,
) -> rquickjs::Result<rquickjs::Object<'js>> {
    let obj = rquickjs::Object::new(ctx.clone())?;

    for flags in &iface.flags {
        let flags_obj = rquickjs::Object::new(ctx.clone())?;
        for (i, name) in flags.names().enumerate() {
            flags_obj.set(name.to_upper_camel_case(), 1u32 << i)?;
        }
        obj.set(flags.name().to_upper_camel_case(), flags_obj)?;
    }

    for enum_ty in &iface.enums {
        let enum_obj = rquickjs::Object::new(ctx.clone())?;
        for (i, name) in enum_ty.names().enumerate() {
            let i = i as u32;
            enum_obj.set(name.to_upper_camel_case(), i)?;
            enum_obj.set(i, name)?;
        }
        obj.set(enum_ty.name().to_upper_camel_case(), enum_obj)?;
    }

    for variant in &iface.variants {
        let variant_obj = rquickjs::Object::new(ctx.clone())?;
        for (i, (name, _payload_ty)) in variant.cases().enumerate() {
            let tag = i as u32;
            let camel = name.to_upper_camel_case();
            variant_obj.set(camel.as_str(), tag)?;
            variant_obj.set(tag, name)?;
        }
        obj.set(variant.name().to_upper_camel_case(), variant_obj)?;
    }

    for func in &iface.funcs {
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

fn register_imports(ctx: &rquickjs::Ctx<'_>, wit_def: Wit) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    for (name, iface) in partition_imports(wit_def).iter() {
        let obj = interface_to_js(ctx, iface)?;
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

fn call_import<'js>(
    ctx: rquickjs::Ctx<'js>,
    func_index: usize,
    args: Vec<Value<'js>>,
) -> rquickjs::Result<Value<'js>> {
    let wit_def = ctx.wit();
    let func = wit_def.import_func(func_index);

    let mut call = QjsCallContext::default();
    for arg in args.into_iter().rev() {
        call.push_value(&ctx, arg);
    }

    if func.is_async() {
        let (promise, resolve, _reject) = ctx.promise()?;

        if let Some(pending) = unsafe { func.call_import_async(&mut call) } {
            let handle = pending.subtask;
            let buffer = pending.buffer;

            let resolve = Persistent::save(&ctx, resolve.into_value());
            let pending = Pending::ImportCall {
                func_index,
                call,
                buffer,
                resolve,
            };
            ctx.task().register(handle, pending);
        } else {
            let result = func
                .result()
                .and_then(|_| call.maybe_pop_persistent())
                .map(|p| p.restore(&ctx))
                .transpose()?
                .unwrap_or_else(|| Value::new_undefined(ctx.clone()));

            resolve
                .call::<_, Value>((result,))
                .expect("Failed to resolve async import");
        }

        Ok(promise.into_value())
    } else {
        func.call_import_sync(&mut call);
        match call.maybe_pop_persistent() {
            Some(persistent) => persistent.restore(&ctx),
            None => Ok(Value::new_undefined(ctx)),
        }
    }
}

/// Build `componentize_js_async_exports` object.
///
/// Each wrapper calls the user's export function, then chains `.then()` to
/// signal `task_return` back to the host.
fn register_async_exports(ctx: &rquickjs::Ctx<'_>, wit_def: Wit) -> rquickjs::Result<()> {
    let exports = rquickjs::Object::new(ctx.clone())?;
    let mut iface_objs: HashMap<String, rquickjs::Object<'_>> = HashMap::new();

    for (func_index, func) in wit_def.iter_export_funcs().enumerate() {
        let func_name = func.name().to_lower_camel_case();
        let iface_name = func.interface().map(|s| s.to_lower_camel_case());

        let fn_name = func_name.clone();
        let iface = iface_name.clone();

        let wrapper = Function::new(
            ctx.clone(),
            coerce_fn(move |ctx: Ctx<'_>, args: Rest<Value<'_>>| {
                let globals = ctx.globals();

                let user_fn: Function = if let Some(ref iface) = iface {
                    let iface_obj: rquickjs::Object = globals.get(iface.as_str())?;
                    iface_obj.get(fn_name.as_str())?
                } else {
                    globals.get(fn_name.as_str())?
                };

                let mut js_args = function::Args::new(ctx.clone(), args.0.len());
                for arg in args.0 {
                    js_args.push_arg(arg)?;
                }
                let result = user_fn.call_arg::<Value>(js_args)?;

                let promise_obj = result
                    .as_object()
                    .ok_or_else(|| rquickjs::Error::new_from_js("value", "promise"))?;

                let then_fn: Function = promise_obj.get("then")?;

                let then_cb = Function::new(
                    ctx.clone(),
                    coerce_fn(move |ctx: Ctx<'_>, args: Rest<Value<'_>>| {
                        let value = args
                            .0
                            .into_iter()
                            .next()
                            .unwrap_or_else(|| Value::new_undefined(ctx.clone()));

                        let func = ctx.wit().export_func(func_index);
                        let mut call = QjsCallContext::default();
                        if func.result().is_some() {
                            call.push_value(&ctx, value);
                        }
                        func.call_task_return(&mut call);
                        Ok(Value::new_undefined(ctx))
                    }),
                )?;

                let catch_cb = Function::new(
                    ctx.clone(),
                    coerce_fn(move |ctx: Ctx<'_>, _args: Rest<Value<'_>>| {
                        let func = ctx.wit().export_func(func_index);
                        let mut call = QjsCallContext::default();
                        func.call_task_return(&mut call);
                        Ok(Value::new_undefined(ctx))
                    }),
                )?;

                let mut call_args = function::Args::new(ctx.clone(), 2);
                call_args.this(result)?;
                call_args.push_arg(then_cb)?;
                call_args.push_arg(catch_cb)?;
                then_fn.call_arg(call_args)
            }),
        )?;

        let target = match &iface_name {
            Some(iface) => iface_objs
                .entry(iface.clone())
                .or_insert_with(|| rquickjs::Object::new(ctx.clone()).unwrap()),
            None => &exports,
        };
        target.set(func_name.as_str(), wrapper)?;
    }

    for (name, obj) in iface_objs {
        exports.set(name.as_str(), obj)?;
    }

    ctx.globals()
        .set("componentize_js_async_exports", exports)?;
    Ok(())
}

/// Register `__componentize_make_stream` and `__componentize_make_future`
/// on globalThis for the js shim to create stream/future pairs.
fn register_stream_future_factories(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    globals.set(
        "__componentize_make_stream",
        Function::new(
            ctx.clone(),
            coerce_fn(move |ctx: Ctx<'_>, args: Rest<Value<'_>>| make_stream(ctx, args)),
        )?,
    )?;

    globals.set(
        "__componentize_make_future",
        Function::new(
            ctx.clone(),
            coerce_fn(move |ctx: Ctx<'_>, args: Rest<Value<'_>>| make_future(ctx, args)),
        )?,
    )?;

    Ok(())
}

/// Register `__componentize_get_memory_usage` and `__componentize_run_gc`
/// on globalThis for memory introspection from JS.
///
fn register_memory_introspection(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    globals.set(
        "__componentize_get_memory_usage",
        Function::new(
            ctx.clone(),
            coerce_fn(
                move |ctx: Ctx<'_>, _args: Rest<Value<'_>>| -> rquickjs::Result<Value<'_>> {
                    let usage = unsafe {
                        let rt = rquickjs::qjs::JS_GetRuntime(ctx.as_raw().as_ptr());
                        let mut usage = std::mem::MaybeUninit::uninit();
                        rquickjs::qjs::JS_ComputeMemoryUsage(rt, usage.as_mut_ptr());
                        usage.assume_init()
                    };
                    let obj = rquickjs::Object::new(ctx.clone())?;
                    obj.set("mallocSize", usage.malloc_size)?;
                    obj.set("mallocCount", usage.malloc_count)?;
                    obj.set("memoryUsedSize", usage.memory_used_size)?;
                    obj.set("objCount", usage.obj_count)?;
                    obj.set("strCount", usage.str_count)?;
                    obj.set("atomCount", usage.atom_count)?;
                    obj.set("atomSize", usage.atom_size)?;
                    obj.set("propCount", usage.prop_count)?;
                    obj.set("shapeCount", usage.shape_count)?;
                    obj.set("arrayCount", usage.array_count)?;
                    Ok(obj.into_value())
                },
            ),
        )?,
    )?;

    globals.set(
        "__componentize_run_gc",
        Function::new(
            ctx.clone(),
            coerce_fn(
                move |ctx: Ctx<'_>, _args: Rest<Value<'_>>| -> rquickjs::Result<Value<'_>> {
                    unsafe {
                        let rt = rquickjs::qjs::JS_GetRuntime(ctx.as_raw().as_ptr());
                        rquickjs::qjs::JS_RunGC(rt);
                    }
                    Ok(Value::new_undefined(ctx))
                },
            ),
        )?,
    )?;

    Ok(())
}
