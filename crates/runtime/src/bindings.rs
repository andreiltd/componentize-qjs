//! WIT to/from JS binding registration.
use heck::{ToLowerCamelCase, ToUpperCamelCase};
use rquickjs::Persistent;
use rquickjs::function::{Constructor, Rest, This};
use rquickjs::{Ctx, Function, Object, Value};
use smallvec::SmallVec;
use wit_dylib_ffi::{Resource, Wit};

use crate::CtxExt;
use crate::futures::{make_future, register_future_classes};
use crate::imports::ImportCall;
use crate::resources::imported_resource_prototype;
use crate::streams::{make_stream, register_stream_classes};
use crate::wit_imports::{FuncKind, WitInterface, classify, find_resource};
use crate::{DetHashSet, DetIndexMap, coerce_fn};

/// Register all wit bindings on the js global scope.
pub(crate) fn register(ctx: &rquickjs::Ctx<'_>, wit_def: Wit) -> rquickjs::Result<()> {
    register_stream_classes(ctx)?;
    register_future_classes(ctx)?;
    register_resource_classes(ctx, wit_def)?;
    register_root_imports(ctx)?;
    register_cqjs_namespace(ctx)?;
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
        Persistent<Constructor<'static>>,
        Persistent<Object<'static>>,
    )> = Vec::new();

    let native_proto = imported_resource_prototype(ctx)?;
    for (index, group) in groups {
        let proto = Object::new(ctx.clone())?;
        proto.set_prototype(Some(&native_proto))?;

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
            proto.set(method.to_lower_camel_case(), js_func)?;
        }

        let class: Constructor = match group.ctor {
            Some(func_index) => Constructor::new_prototype(
                ctx,
                proto.clone(),
                move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
                    call_import(ctx, func_index, SmallVec::from_vec(args.0))
                },
            )?,
            None => {
                let resource_name = group.resource.name();
                Constructor::new_prototype(
                    ctx,
                    proto.clone(),
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
            Persistent::save(ctx, class),
            Persistent::save(ctx, proto),
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

fn register_root_imports(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    let imports = ctx.wit_import_registry();
    let obj = interface_to_js(ctx, imports.root())?;

    for key in obj.keys::<String>() {
        let key = key?;
        let val: Value = obj.get(&key)?;
        globals.set(key, val)?;
    }

    Ok(())
}

/// Adapt a JS binding invocation to its validated canonical import call.
fn call_import<'js>(
    ctx: rquickjs::Ctx<'js>,
    func_index: usize,
    args: SmallVec<[Value<'js>; 8]>,
) -> rquickjs::Result<Value<'js>> {
    ImportCall::new(&ctx, ctx.wit().import_func(func_index), args)?.invoke(&ctx)
}

/// Register the `__cqjs` namespace object on globalThis.
///
/// Consolidates all internal bridge globals into a single frozen object:
/// - `makeStream(typeIndex)` — create a stream pair
/// - `makeFuture(typeIndex)` — create a future pair
/// - `getMemoryUsage()` — return QuickJS memory statistics
/// - `runGc()` — trigger QuickJS garbage collection
fn register_cqjs_namespace(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()> {
    let ns = rquickjs::Object::new(ctx.clone())?;

    // Stream/future factories
    ns.set("makeStream", Function::new(ctx.clone(), make_stream)?)?;

    ns.set("makeFuture", Function::new(ctx.clone(), make_future)?)?;

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
                    ctx.run_gc();
                    crate::resources::drain_resource_drops(&ctx);
                    Ok(Value::new_undefined(ctx))
                },
            ),
        )?,
    )?;

    // Freeze and install on globalThis
    let object_ctor: rquickjs::Object = ctx.globals().get("Object")?;
    let freeze_fn: Function = object_ctor.get("freeze")?;
    freeze_fn.call::<_, Value>((ns.clone(),))?;

    ctx.globals().set("__cqjs", ns)?;
    Ok(())
}
