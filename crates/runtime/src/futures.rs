//! component-model future operations using rquickjs classes.
use rquickjs::class::{Class, JsClass, Trace};
use rquickjs::function::This;
use rquickjs::{CatchResultExt, Ctx, Function, Object, Persistent, Promise, Value};
use rquickjs::{JsLifetime, function};

use crate::CtxExt;
use crate::abi::{CopyResult, is_blocked_raw, unpack_copy_result};
use crate::buffer::BufferGuard;
use crate::endpoint::CopyEnd;
use crate::result::JsCompletion;
use crate::task::Pending;
use crate::{QjsCallContext, resolve_promise, symbol_dispose, with_ctx};

#[derive(Trace, JsLifetime)]
pub(crate) struct FutureReadable<'js> {
    #[qjs(skip_trace)]
    pub(crate) end: CopyEnd,
    promise: Option<Promise<'js>>,
}

impl FutureReadable<'_> {
    fn new(type_index: u32, handle: u32) -> Self {
        Self {
            end: CopyEnd::new_future(type_index, handle),
            promise: None,
        }
    }
}

impl<'js> JsClass<'js> for FutureReadable<'js> {
    const NAME: &'static str = "FutureReadable";
    type Mutable = rquickjs::class::Writable;

    fn prototype(ctx: &Ctx<'js>) -> rquickjs::Result<Option<Object<'js>>> {
        let proto = Object::new(ctx.clone())?;
        proto.set("read", Function::new(ctx.clone(), future_read)?)?;
        proto.set(
            "cancelRead",
            Function::new(ctx.clone(), future_cancel_read)?,
        )?;

        let drop_fn = Function::new(ctx.clone(), future_drop_readable)?;
        proto.set("drop", drop_fn.clone())?;

        let dispose_sym = symbol_dispose(ctx)?;
        proto.set(dispose_sym, drop_fn)?;

        Ok(Some(proto))
    }

    fn constructor(_ctx: &Ctx<'js>) -> rquickjs::Result<Option<function::Constructor<'js>>> {
        Ok(None)
    }
}

#[derive(Trace, JsLifetime)]
pub(crate) struct FutureWritable {
    #[qjs(skip_trace)]
    pub(crate) end: CopyEnd,
}

impl FutureWritable {
    fn new(type_index: u32, handle: u32) -> Self {
        Self {
            end: CopyEnd::new_future(type_index, handle),
        }
    }
}

impl<'js> JsClass<'js> for FutureWritable {
    const NAME: &'static str = "FutureWritable";
    type Mutable = rquickjs::class::Writable;

    fn prototype(ctx: &Ctx<'js>) -> rquickjs::Result<Option<Object<'js>>> {
        let proto = Object::new(ctx.clone())?;
        proto.set("write", Function::new(ctx.clone(), future_write)?)?;
        proto.set(
            "cancelWrite",
            Function::new(ctx.clone(), future_cancel_write)?,
        )?;

        let drop_fn = Function::new(ctx.clone(), future_drop_writable)?;
        proto.set("drop", drop_fn.clone())?;

        let dispose_sym = symbol_dispose(ctx)?;
        proto.set(dispose_sym, drop_fn)?;

        Ok(Some(proto))
    }

    fn constructor(_ctx: &Ctx<'js>) -> rquickjs::Result<Option<function::Constructor<'js>>> {
        Ok(None)
    }
}

pub(crate) fn register_future_classes(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    Class::<FutureReadable>::define(&ctx.globals())?;
    Class::<FutureWritable>::define(&ctx.globals())?;
    Ok(())
}

pub(crate) fn make_future_readable<'js>(
    ctx: &Ctx<'js>,
    type_index: u32,
    handle: u32,
) -> rquickjs::Result<Object<'js>> {
    let instance = Class::instance(ctx.clone(), FutureReadable::new(type_index, handle))?;
    Ok(instance.into_inner())
}

pub(crate) fn make_future<'js>(ctx: Ctx<'js>, type_index: u32) -> rquickjs::Result<Value<'js>> {
    let ty = ctx
        .wit()
        .iter_futures()
        .nth(type_index as usize)
        .ok_or_else(|| rquickjs::Exception::throw_range(&ctx, "unknown WIT future type"))?;

    let handles = unsafe { ty.new()() };
    let tx_handle = (handles >> 32) as u32;
    let rx_handle = (handles & 0xFFFF_FFFF) as u32;

    let tx = Class::instance(ctx.clone(), FutureWritable::new(type_index, tx_handle))?;
    let rx = make_future_readable(&ctx, type_index, rx_handle)?;

    let result = rquickjs::Object::new(ctx)?;
    result.set("writable", tx.into_inner())?;
    result.set("readable", rx)?;

    Ok(result.into_value())
}

pub(crate) fn lower_thenable<'js>(
    ctx: &Ctx<'js>,
    type_index: u32,
    thenable: Value<'js>,
) -> rquickjs::Result<u32> {
    if !ctx.task().is_active() {
        return Err(rquickjs::Error::new_from_js(
            "thenable",
            "WIT future lowering requires an active async call",
        ));
    }

    let wit: Object = ctx.globals().get("wit")?;
    let future: Function = wit.get("Future")?;
    let from: Function = future.get("from")?;
    let pair: Object = from.call((thenable, type_index))?;
    let readable: Value = pair.get("readable")?;
    let readable = Class::<FutureReadable>::from_value(&readable)?;
    let handle = readable.borrow_mut().end.begin_transfer(type_index)?;

    Ok(handle)
}

fn future_read<'js>(
    this: This<Class<'js, FutureReadable<'js>>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    {
        let readable = this.0.borrow();
        if !readable.end.has_handle() {
            return Err(rquickjs::Exception::throw_type(
                &ctx,
                "future already dropped or transferred",
            ));
        }
        if let Some(promise) = &readable.promise {
            return Ok(promise.clone().into_value());
        }
    }

    ctx.task().ensure_active(&ctx)?;
    let (handle, type_index) = this.0.borrow().end.begin_op()?;

    let (promise, resolve, reject) = ctx.promise()?;
    this.0.borrow_mut().promise = Some(promise.clone());
    let ty = ctx.wit().future(type_index as usize);

    let buffer = BufferGuard::new_zeroed(ty.abi_payload_size(), ty.abi_payload_align());
    let mut call = QjsCallContext::default();

    let code = unsafe { ty.read()(handle, buffer.ptr().cast()) };

    if is_blocked_raw(code) {
        this.0.borrow_mut().end.mark_blocked();
        let pending = Pending::FutureRead {
            call,
            buffer,
            resolve: Persistent::save(&ctx, resolve),
            reject: Persistent::save(&ctx, reject),
            wrapper: Persistent::save(&ctx, this.0.into_inner().into_value()),
        };

        ctx.task().register(handle, pending);
    } else {
        finish_read(&ctx, &this.0, &mut call, buffer, code)?.settle(&resolve, &reject)?;
    }

    Ok(promise.into_value())
}

/// Finish any future read: update its cached promise, then lift or reject.
///
/// The ABI buffer is released after lifting. The caller retains the conversion
/// context through promise settlement so its resource guards remain alive.
fn finish_read<'js>(
    ctx: &Ctx<'js>,
    class: &Class<'js, FutureReadable<'js>>,
    call: &mut QjsCallContext,
    buffer: BufferGuard,
    code: u32,
) -> rquickjs::Result<JsCompletion<'js>> {
    let (_, result) = unpack_copy_result(code).expect("future read completion must not block");
    let type_index = {
        let mut readable = class.borrow_mut();
        readable.end.mark_completed(result);

        if result == CopyResult::Cancelled {
            readable.promise = None;
        }

        readable.end.type_index()
    };

    let completion = match result {
        CopyResult::Completed => {
            let ty = ctx.wit().future(type_index as usize);
            unsafe { ty.lift(call, buffer.ptr()) };

            let val = call
                .maybe_pop_value(ctx)?
                .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
            JsCompletion::Return(val)
        }
        CopyResult::Dropped | CopyResult::Cancelled => {
            let message = if result == CopyResult::Dropped {
                "future writer dropped"
            } else {
                "future read cancelled"
            };

            let reason = rquickjs::String::from_str(ctx.clone(), message)?.into_value();
            JsCompletion::Throw(reason)
        }
    };

    drop(buffer);
    Ok(completion)
}

/// Finish any future write, committing ownership only if its value was consumed.
fn finish_write<'js>(
    ctx: &Ctx<'js>,
    class: &Class<'js, FutureWritable>,
    call: &mut QjsCallContext,
    buffer: BufferGuard,
    code: u32,
) -> Value<'js> {
    let (_, result) = unpack_copy_result(code).expect("future write completion must not block");
    let success = result == CopyResult::Completed;
    drop(buffer);

    call.complete_transfers(usize::from(success));
    class.borrow_mut().end.mark_completed(result);
    Value::new_bool(ctx.clone(), success)
}

pub(crate) fn future_cancel_read<'js>(
    this: This<Class<'js, FutureReadable<'js>>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (handle, type_index) = this.0.borrow().end.begin_cancel()?;
    let ty = ctx.wit().future(type_index as usize);

    ctx.task().unjoin(handle);
    let code = unsafe { ty.cancel_read()(handle) };

    match unpack_copy_result(code) {
        None => {
            ctx.task().rejoin(handle);
            this.0.borrow_mut().end.mark_cancel_blocked();
            Ok(Value::new_undefined(ctx))
        }
        Some((_progress, result)) => {
            handle_read_event(handle, code);
            Ok(Value::new_number(ctx, result as u32 as f64))
        }
    }
}

fn future_drop_readable<'js>(
    this: This<Class<'js, FutureReadable<'js>>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<()> {
    let (handle, type_index) = {
        let mut readable = this.0.borrow_mut();
        let handle = readable.end.begin_drop()?;
        readable.promise = None;
        (handle, readable.end.type_index())
    };

    if let Some(handle) = handle {
        let ty = ctx.wit().future(type_index as usize);
        unsafe { ty.drop_readable()(handle) };
    }

    Ok(())
}

fn future_write<'js>(
    this: This<Class<'js, FutureWritable>>,
    ctx: Ctx<'js>,
    value: Value<'js>,
) -> rquickjs::Result<Value<'js>> {
    ctx.task().ensure_active(&ctx)?;
    let (handle, type_index) = this.0.borrow().end.begin_op()?;

    let (promise, resolve, _reject) = ctx.promise()?;
    let ty = ctx.wit().future(type_index as usize);

    let buffer = BufferGuard::new_zeroed(ty.abi_payload_size(), ty.abi_payload_align());

    let mut call = QjsCallContext::default();
    call.push_value(&ctx, value);
    unsafe { ty.lower(&mut call, buffer.ptr()) };

    let code = unsafe { ty.write()(handle, buffer.ptr().cast()) };

    if is_blocked_raw(code) {
        this.0.borrow_mut().end.mark_blocked();
        let pending = Pending::FutureWrite {
            call,
            buffer,
            resolve: Persistent::save(&ctx, resolve),
            wrapper: Persistent::save(&ctx, this.0.into_inner().into_value()),
        };
        ctx.task().register(handle, pending);
    } else {
        let result = finish_write(&ctx, &this.0, &mut call, buffer, code);

        resolve
            .call::<_, Value>((result,))
            .expect("resolve future write");
    }

    Ok(promise.into_value())
}

pub(crate) fn future_cancel_write<'js>(
    this: This<Class<'js, FutureWritable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (handle, type_index) = this.0.borrow().end.begin_cancel()?;
    let ty = ctx.wit().future(type_index as usize);
    ctx.task().unjoin(handle);
    let code = unsafe { ty.cancel_write()(handle) };

    match unpack_copy_result(code) {
        None => {
            ctx.task().rejoin(handle);
            this.0.borrow_mut().end.mark_cancel_blocked();
            Ok(Value::new_undefined(ctx))
        }
        Some((_progress, result)) => {
            handle_write_event(handle, code);
            Ok(Value::new_number(ctx, result as u32 as f64))
        }
    }
}

fn future_drop_writable<'js>(
    this: This<Class<'js, FutureWritable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<()> {
    let (handle, type_index) = {
        let mut writable = this.0.borrow_mut();
        (writable.end.begin_drop()?, writable.end.type_index())
    };

    if let Some(handle) = handle {
        let ty = ctx.wit().future(type_index as usize);
        unsafe { ty.drop_writable()(handle) };
    }
    Ok(())
}

/// Handle a future-write completion event in the async callback.
pub(crate) fn handle_write_event(handle: u32, result: u32) {
    let pending = with_ctx(|ctx| ctx.task().take(handle));

    let Pending::FutureWrite {
        mut call,
        buffer,
        resolve,
        wrapper,
    } = pending
    else {
        unreachable!("expected FutureWrite pending");
    };

    let result = with_ctx(|ctx| {
        let w = wrapper.restore(ctx).unwrap();
        let class = Class::<FutureWritable>::from_value(&w).unwrap();
        let value = finish_write(ctx, &class, &mut call, buffer, result);
        Some(Persistent::save(ctx, value))
    });

    resolve_promise(resolve, result);
}

/// Handle a future-read completion event in the async callback.
pub(crate) fn handle_read_event(handle: u32, result: u32) {
    let pending = with_ctx(|ctx| ctx.task().take(handle));

    let Pending::FutureRead {
        mut call,
        buffer,
        resolve,
        reject,
        wrapper,
    } = pending
    else {
        unreachable!("expected FutureRead pending");
    };

    with_ctx(|ctx| {
        let value = wrapper.restore(ctx).unwrap();
        let class = Class::<FutureReadable>::from_value(&value).unwrap();

        finish_read(ctx, &class, &mut call, buffer, result)
            .catch(ctx)
            .expect("Failed to finish future read")
            .settle_persistent(ctx, resolve, reject);
    });
}
