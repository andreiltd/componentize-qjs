//! WIT to/from JS binding registration.
use heck::{ToLowerCamelCase, ToUpperCamelCase};
use rquickjs::Persistent;
use rquickjs::function;
use rquickjs::function::{Constructor, Rest, This};
use rquickjs::{Ctx, Function, Object, Value};
use smallvec::SmallVec;
use wit_dylib_ffi::{Resource, Wit};

use crate::CtxExt;
use crate::futures::{make_future, register_future_classes};
use crate::streams::{make_stream, register_stream_classes};
use crate::task::Pending;
use crate::trivia::iface_lookup;
use crate::wit_imports::{FuncKind, WitInterface, classify, find_resource, root_bindings};
use crate::{DetHashSet, DetIndexMap, QjsCallContext, coerce_fn};

/// Register all wit bindings on the js global scope.
pub(crate) fn register(ctx: &rquickjs::Ctx<'_>, wit_def: Wit) -> rquickjs::Result<()> {
    register_stream_classes(ctx)?;
    register_future_classes(ctx)?;
    register_resource_classes(ctx, wit_def)?;
    register_root_imports(ctx, wit_def)?;
    register_cqjs_namespace(ctx, wit_def)?;
    Ok(())
}

/// Build a JS "class" (constructor + prototype) for every imported resource.
fn register_resource_classes<'js>(ctx: &Ctx<'js>, wit: Wit) -> rquickjs::Result<()> {
    struct Group {
        resource: Resource,
        ctor: Option<usize>,
        methods: Vec<(&'static str, usize)>,
        statics: Vec<(&'static str, usize)>,
    }

    let mut groups: DetIndexMap<usize, Group> = DetIndexMap::default();

    for func in wit.iter_import_funcs() {
        let kind = classify(func.name());
        let resource_name = match kind {
            FuncKind::Freestanding => continue,
            FuncKind::Constructor { resource }
            | FuncKind::Method { resource, .. }
            | FuncKind::Static { resource, .. } => resource,
        };

        let Some(resource) = find_resource(wit, func.interface(), resource_name) else {
            continue;
        };

        // Only imported resources get host-backed classes; exported (JS-backed)
        // resources have a `rep` and are handled on the export side.
        if resource.rep().is_some() {
            continue;
        }

        let group = groups.entry(resource.index()).or_insert_with(|| Group {
            resource,
            ctor: None,
            methods: Vec::new(),
            statics: Vec::new(),
        });

        match kind {
            FuncKind::Constructor { .. } => group.ctor = Some(func.index()),
            FuncKind::Method { method, .. } => group.methods.push((method, func.index())),
            FuncKind::Static { method, .. } => group.statics.push((method, func.index())),
            FuncKind::Freestanding => unreachable!(),
        }
    }

    let mut built: Vec<(
        usize,
        Persistent<Value<'static>>,
        Persistent<Value<'static>>,
    )> = Vec::new();

    for (index, group) in groups {
        let prototype = Object::new(ctx.clone())?;
        for (method, func_index) in group.methods {
            let js_func = Function::new(
                ctx.clone(),
                move |this: This<Value<'js>>, ctx: Ctx<'js>, args: Rest<Value<'js>>| {
                    let mut call_args: SmallVec<[Value<'js>; 8]> =
                        SmallVec::with_capacity(args.0.len() + 1);
                    call_args.push(this.0);
                    call_args.extend(args.0);
                    call_import(ctx, func_index, call_args)
                },
            )?;
            prototype.set(method.to_lower_camel_case(), js_func)?;
        }

        let class: Constructor = match group.ctor {
            Some(func_index) => Constructor::new_prototype(
                ctx,
                prototype.clone(),
                move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
                    call_import(ctx, func_index, SmallVec::from_vec(args.0))
                },
            )?,
            None => {
                let resource_name = group.resource.name();
                Constructor::new_prototype(
                    ctx,
                    prototype.clone(),
                    move |ctx: Ctx<'js>, _args: Rest<Value<'js>>| -> rquickjs::Result<Value<'js>> {
                        Err(rquickjs::Exception::throw_type(
                            &ctx,
                            &format!("{resource_name} has no constructor"),
                        ))
                    },
                )?
            }
        };

        for (method, func_index) in group.statics {
            let js_func =
                Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
                    call_import(ctx, func_index, SmallVec::from_vec(args.0))
                })?;
            class.set(method.to_lower_camel_case(), js_func)?;
        }

        built.push((
            index,
            Persistent::save(ctx, class.into_value()),
            Persistent::save(ctx, prototype.into_value()),
        ));
    }

    let registry = ctx.resource_classes();
    for (index, class, prototype) in built {
        registry.insert(index, class, prototype);
    }

    Ok(())
}

/// Create a js object containing all functions, flags, enums, and variants
/// for a single wit interface.
pub(crate) fn interface_to_js<'js>(
    ctx: &rquickjs::Ctx<'js>,
    iface: &WitInterface,
) -> rquickjs::Result<rquickjs::Object<'js>> {
    let obj = rquickjs::Object::new(ctx.clone())?;

    let mut seen_resources: DetHashSet<usize> = DetHashSet::default();
    for func in &iface.funcs {
        match classify(func.name()) {
            FuncKind::Freestanding => {
                let func_name = func.name().to_lower_camel_case();
                let func_index = func.index();
                let js_func = rquickjs::Function::new(
                    ctx.clone(),
                    move |ctx: rquickjs::Ctx<'js>, args: Rest<Value<'js>>| {
                        call_import(ctx, func_index, SmallVec::from_vec(args.0))
                    },
                )?;
                obj.set(func_name, js_func)?;
            }
            FuncKind::Constructor { resource }
            | FuncKind::Method { resource, .. }
            | FuncKind::Static { resource, .. } => {
                let Some(res) = find_resource(ctx.wit(), func.interface(), resource) else {
                    continue;
                };
                if !seen_resources.insert(res.index()) {
                    continue;
                }
                if let Some(class) = ctx.resource_classes().class(res.index()) {
                    obj.set(resource.to_upper_camel_case(), class.restore(ctx)?)?;
                }
            }
        }
    }

    Ok(obj)
}

fn register_root_imports(ctx: &rquickjs::Ctx<'_>, wit_def: Wit) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    let obj = interface_to_js(ctx, &root_bindings(wit_def))?;

    for key in obj.keys::<String>() {
        let key = key?;
        let val: Value = obj.get(&key)?;
        globals.set(key, val)?;
    }

    Ok(())
}

fn call_import<'js>(
    ctx: rquickjs::Ctx<'js>,
    func_index: usize,
    args: SmallVec<[Value<'js>; 8]>,
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

/// Build the `asyncExports` object for the `__cqjs` namespace.
///
/// Each wrapper calls the user's export function, then chains `.then()` to
/// signal `task_return` back to the host.
fn build_async_exports<'js>(
    ctx: &rquickjs::Ctx<'js>,
    wit_def: Wit,
) -> rquickjs::Result<rquickjs::Object<'js>> {
    let exports = rquickjs::Object::new(ctx.clone())?;
    // Insertion-ordered so the resulting object's property order is deterministic
    // (and follows WIT declaration order) for a reproducible Wizer snapshot.
    let mut iface_objs: DetIndexMap<String, rquickjs::Object<'_>> = DetIndexMap::default();

    for (func_index, func) in wit_def.iter_export_funcs().enumerate() {
        let func_name = func.name().to_lower_camel_case();
        let iface_name = func
            .interface()
            .map(|interface| iface_lookup(ctx, interface).to_string());

        let fn_name = func_name.clone();
        let iface = iface_name.clone();

        let wrapper = Function::new(
            ctx.clone(),
            coerce_fn(move |ctx: Ctx<'_>, args: Rest<Value<'_>>| {
                let exports = ctx.user_module().exports(&ctx)?;

                let user_fn: Function = if let Some(ref iface) = iface {
                    let iface_obj: rquickjs::Object = exports.get(iface.as_str())?;
                    iface_obj.get(fn_name.as_str())?
                } else {
                    exports.get(fn_name.as_str())?
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
                    coerce_fn(move |ctx: Ctx<'_>, args: Rest<Value<'_>>| {
                        let reason = args
                            .0
                            .into_iter()
                            .next()
                            .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                        let msg = reason
                            .as_object()
                            .and_then(|obj| obj.get::<_, rquickjs::String>("message").ok())
                            .and_then(|s| s.to_string().ok())
                            .unwrap_or_else(|| format!("{reason:?}"));
                        panic!("async export rejected: {msg}");
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

    Ok(exports)
}

/// Register the `__cqjs` namespace object on globalThis.
///
/// Consolidates all internal bridge globals into a single frozen object:
/// - `makeStream(typeIndex)` — create a stream pair
/// - `makeFuture(typeIndex)` — create a future pair
/// - `getMemoryUsage()` — return QuickJS memory statistics
/// - `runGc()` — trigger QuickJS garbage collection
/// - `asyncExports` — object containing async export wrappers
fn register_cqjs_namespace(ctx: &rquickjs::Ctx<'_>, wit_def: Wit) -> rquickjs::Result<()> {
    let ns = rquickjs::Object::new(ctx.clone())?;

    // Stream/future factories
    ns.set(
        "makeStream",
        Function::new(
            ctx.clone(),
            coerce_fn(move |ctx: Ctx<'_>, args: Rest<Value<'_>>| make_stream(ctx, args)),
        )?,
    )?;

    ns.set(
        "makeFuture",
        Function::new(
            ctx.clone(),
            coerce_fn(move |ctx: Ctx<'_>, args: Rest<Value<'_>>| make_future(ctx, args)),
        )?,
    )?;

    // Memory introspection
    ns.set(
        "getMemoryUsage",
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

    ns.set(
        "runGc",
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

    // Async export wrappers
    let async_exports = build_async_exports(ctx, wit_def)?;
    ns.set("asyncExports", async_exports)?;

    // Freeze and install on globalThis
    let object_ctor: rquickjs::Object = ctx.globals().get("Object")?;
    let freeze_fn: Function = object_ctor.get("freeze")?;
    freeze_fn.call::<_, Value>((ns.clone(),))?;

    ctx.globals().set("__cqjs", ns)?;
    Ok(())
}
