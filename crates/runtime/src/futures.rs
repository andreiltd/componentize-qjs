//! component-model future operations using rquickjs classes.
use rquickjs::class::{Class, JsClass, Trace};
use rquickjs::function::This;
use rquickjs::{Ctx, Function, Object, Persistent, Value};
use rquickjs::{JsLifetime, function};

use crate::CtxExt;
use crate::abi::{CopyEnd, CopyResult, is_blocked_raw, unpack_copy_result};
use crate::buffer::BufferGuard;
use crate::task::Pending;
use crate::{QjsCallContext, resolve_promise, symbol_dispose, with_ctx};

#[derive(Trace, JsLifetime)]
pub(crate) struct FutureReadable {
    #[qjs(skip_trace)]
    pub(crate) end: CopyEnd,
}

impl FutureReadable {
    fn new(type_index: u32, handle: u32) -> Self {
        Self {
            end: CopyEnd::new_future(type_index, handle),
        }
    }
}

impl<'js> JsClass<'js> for FutureReadable {
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

pub(crate) fn make_future<'js>(
    ctx: Ctx<'js>,
    args: function::Rest<Value<'js>>,
) -> rquickjs::Result<Value<'js>> {
    let type_index: u32 = args.0[0].get()?;
    let ty = ctx.wit().future(type_index as usize);

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

fn future_read<'js>(
    this: This<Class<'js, FutureReadable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (handle, type_index) = this.0.borrow().end.begin_op()?;

    let (promise, resolve, reject) = ctx.promise()?;
    let ty = ctx.wit().future(type_index as usize);

    let buffer = BufferGuard::new_zeroed(ty.abi_payload_size(), ty.abi_payload_align());
    let mut call = QjsCallContext::default();

    let code = unsafe { ty.read()(handle, buffer.ptr().cast()) };

    if is_blocked_raw(code) {
        this.0.borrow_mut().end.mark_blocked();
        let pending = Pending::FutureRead {
            call,
            buffer,
            resolve: Persistent::save(&ctx, resolve.into_value()),
            reject: Persistent::save(&ctx, reject.into_value()),
            wrapper: Persistent::save(&ctx, this.0.into_inner().into_value()),
        };

        ctx.task().register(handle, pending);
    } else {
        let result_code = CopyResult::try_from(code & 0xF).expect("unknown copy result");
        this.0.borrow_mut().end.mark_completed(result_code);

        match result_code {
            CopyResult::Completed => {
                unsafe { ty.lift(&mut call, buffer.ptr()) };
                let result = call.pop_value(&ctx);
                resolve
                    .call::<_, Value>((result,))
                    .expect("resolve future read");
            }
            CopyResult::Dropped => {
                let msg =
                    rquickjs::String::from_str(ctx.clone(), "future writer dropped")?.into_value();
                reject.call::<_, Value>((msg,)).ok();
            }
            CopyResult::Cancelled => {
                let msg =
                    rquickjs::String::from_str(ctx.clone(), "future read cancelled")?.into_value();
                reject.call::<_, Value>((msg,)).ok();
            }
        }
        drop(buffer);
    }

    Ok(promise.into_value())
}

fn future_cancel_read<'js>(
    this: This<Class<'js, FutureReadable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (handle, type_index) = this.0.borrow().end.begin_cancel()?;
    let ty = ctx.wit().future(type_index as usize);
    let code = unsafe { ty.cancel_read()(handle) };

    match unpack_copy_result(code) {
        None => {
            this.0.borrow_mut().end.mark_cancel_blocked();
            Ok(Value::new_undefined(ctx))
        }
        Some((_progress, result)) => {
            this.0.borrow_mut().end.mark_completed(result);
            Ok(Value::new_number(ctx, result as u32 as f64))
        }
    }
}

fn future_drop_readable<'js>(
    this: This<Class<'js, FutureReadable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<()> {
    let mut w = this.0.borrow_mut();
    if let Some(handle) = w.end.handle.take() {
        let ty = ctx.wit().future(w.end.type_index as usize);
        unsafe { ty.drop_readable()(handle) };
    }
    Ok(())
}

fn future_write<'js>(
    this: This<Class<'js, FutureWritable>>,
    ctx: Ctx<'js>,
    value: Value<'js>,
) -> rquickjs::Result<Value<'js>> {
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
            buffer,
            resolve: Persistent::save(&ctx, resolve.into_value()),
            wrapper: Persistent::save(&ctx, this.0.into_inner().into_value()),
        };
        ctx.task().register(handle, pending);
    } else {
        drop(buffer);
        let result_code = CopyResult::try_from(code & 0xF).expect("unknown copy result");
        let success = result_code == CopyResult::Completed;

        this.0.borrow_mut().end.mark_completed(result_code);
        let result = Value::new_bool(ctx.clone(), success);

        resolve
            .call::<_, Value>((result,))
            .expect("resolve future write");
    }

    Ok(promise.into_value())
}

fn future_cancel_write<'js>(
    this: This<Class<'js, FutureWritable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (handle, type_index) = this.0.borrow().end.begin_cancel()?;
    let ty = ctx.wit().future(type_index as usize);
    let code = unsafe { ty.cancel_write()(handle) };

    match unpack_copy_result(code) {
        None => {
            this.0.borrow_mut().end.mark_cancel_blocked();
            Ok(Value::new_undefined(ctx))
        }
        Some((_progress, result)) => {
            this.0.borrow_mut().end.mark_completed(result);
            Ok(Value::new_number(ctx, result as u32 as f64))
        }
    }
}

fn future_drop_writable<'js>(
    this: This<Class<'js, FutureWritable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<()> {
    let mut w = this.0.borrow_mut();
    if let Some(handle) = w.end.handle.take() {
        let ty = ctx.wit().future(w.end.type_index as usize);
        unsafe { ty.drop_writable()(handle) };
    }
    Ok(())
}

/// Handle a future-write completion event in the async callback.
pub(crate) fn handle_write_event(handle: u32, result: u32) {
    let pending = with_ctx(|ctx| ctx.task().take(handle));

    let Pending::FutureWrite {
        resolve, wrapper, ..
    } = pending
    else {
        unreachable!("expected FutureWrite pending");
    };

    let copy_result = CopyResult::try_from(result & 0xF).expect("unknown copy result");
    let success = copy_result == CopyResult::Completed;

    let result = with_ctx(|ctx| {
        let w = wrapper.restore(ctx).unwrap();
        let class = Class::<FutureWritable>::from_value(&w).unwrap();
        class.borrow_mut().end.mark_completed(copy_result);

        let val = Value::new_bool(ctx.clone(), success);
        let res = Persistent::save(ctx, val);
        Some(res)
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

    let copy_result = CopyResult::try_from(result & 0xF).expect("unknown copy result");

    match copy_result {
        CopyResult::Completed => {
            let result = with_ctx(|ctx| {
                let w = wrapper.restore(ctx).unwrap();
                let class = Class::<FutureReadable>::from_value(&w).unwrap();
                class.borrow_mut().end.mark_completed(copy_result);

                let type_index = class.borrow().end.type_index;
                let ty = ctx.wit().future(type_index as usize);
                unsafe { ty.lift(&mut call, buffer.ptr()) };

                Some(call.pop_persistent())
            });

            drop(buffer);
            resolve_promise(resolve, result);
        }
        CopyResult::Dropped | CopyResult::Cancelled => {
            drop(buffer);
            with_ctx(|ctx| {
                let w = wrapper.restore(ctx).unwrap();
                let class = Class::<FutureReadable>::from_value(&w).unwrap();
                class.borrow_mut().end.mark_completed(copy_result);

                let reject_fn = reject.restore(ctx).unwrap();
                let reject_fn: rquickjs::Function = reject_fn.get().unwrap();

                let msg = if copy_result == CopyResult::Dropped {
                    "future writer dropped"
                } else {
                    "future read cancelled"
                };
                let msg_val = rquickjs::String::from_str(ctx.clone(), msg)
                    .unwrap()
                    .into_value();
                reject_fn.call::<_, Value>((msg_val,)).ok();
            });
        }
    }
}
