//! WASI component-model stream operations using rquickjs classes.
//!
//! Stream endpoints are represented as native JS classes (`StreamReadable`,
//! `StreamWritable`) whose state lives on the Rust side.  Methods on the
//! shared prototype avoid per-instance closure allocations.
#![allow(unsafe_code)]
use crate::CtxExt;
use crate::abi::{CopyEnd, CopyState, is_blocked_raw, unpack_copy_result};
use crate::buffer::BufferGuard;
use crate::task::Pending;
use crate::{QjsCallContext, resolve_promise, symbol_dispose, with_ctx};

use rquickjs::JsLifetime;
use rquickjs::class::{Class, JsClass, Trace};
use rquickjs::function::{self, Rest, This};
use rquickjs::{Ctx, Function, Object, Persistent, Value};

use std::cell::Cell;

/// Rust side state for the readable end of a component-model stream.
#[derive(Trace, JsLifetime)]
pub(crate) struct StreamReadable {
    #[qjs(skip_trace)]
    pub(crate) end: CopyEnd,
}

impl StreamReadable {
    fn new(type_index: u32, handle: u32) -> Self {
        Self {
            end: CopyEnd::new_stream(type_index, handle),
        }
    }
}

impl<'js> JsClass<'js> for StreamReadable {
    const NAME: &'static str = "StreamReadable";
    type Mutable = rquickjs::class::Writable;

    fn prototype(ctx: &Ctx<'js>) -> rquickjs::Result<Option<Object<'js>>> {
        let proto = Object::new(ctx.clone())?;
        proto.set("read", Function::new(ctx.clone(), stream_read)?)?;
        proto.set(
            "cancelRead",
            Function::new(ctx.clone(), stream_cancel_read)?,
        )?;

        let drop_fn = Function::new(ctx.clone(), stream_drop_readable)?;
        proto.set("drop", drop_fn.clone())?;

        let dispose_sym = symbol_dispose(ctx)?;
        proto.set(dispose_sym, drop_fn)?;
        Ok(Some(proto))
    }

    fn constructor(_ctx: &Ctx<'js>) -> rquickjs::Result<Option<function::Constructor<'js>>> {
        Ok(None)
    }
}

/// Rust side state for the writable end of a component-model stream.
#[derive(Trace, JsLifetime)]
pub(crate) struct StreamWritable {
    #[qjs(skip_trace)]
    pub(crate) end: CopyEnd,
}

impl StreamWritable {
    fn new(type_index: u32, handle: u32) -> Self {
        Self {
            end: CopyEnd::new_stream(type_index, handle),
        }
    }
}

impl<'js> JsClass<'js> for StreamWritable {
    const NAME: &'static str = "StreamWritable";
    type Mutable = rquickjs::class::Writable;

    fn prototype(ctx: &Ctx<'js>) -> rquickjs::Result<Option<Object<'js>>> {
        let proto = Object::new(ctx.clone())?;
        proto.set("write", Function::new(ctx.clone(), stream_write)?)?;
        proto.set("writeAll", Function::new(ctx.clone(), stream_write_all)?)?;
        proto.set(
            "cancelWrite",
            Function::new(ctx.clone(), stream_cancel_write)?,
        )?;

        let drop_fn = Function::new(ctx.clone(), stream_drop_writable)?;
        proto.set("drop", drop_fn.clone())?;

        let dispose_sym = symbol_dispose(ctx)?;
        proto.set(dispose_sym, drop_fn)?;

        Ok(Some(proto))
    }

    fn constructor(_ctx: &Ctx<'js>) -> rquickjs::Result<Option<function::Constructor<'js>>> {
        Ok(None)
    }
}

pub(crate) fn register_stream_classes(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    Class::<StreamReadable>::define(&ctx.globals())?;
    Class::<StreamWritable>::define(&ctx.globals())?;
    Ok(())
}

/// Create a `StreamReadable` JS class instance.
pub(crate) fn make_stream_readable<'js>(
    ctx: &Ctx<'js>,
    type_index: u32,
    handle: u32,
) -> rquickjs::Result<Object<'js>> {
    let instance = Class::instance(ctx.clone(), StreamReadable::new(type_index, handle))?;
    Ok(instance.into_inner())
}

/// Create a `[StreamWritable, StreamReadable]` pair.
pub(crate) fn make_stream<'js>(
    ctx: Ctx<'js>,
    args: Rest<Value<'js>>,
) -> rquickjs::Result<Value<'js>> {
    let type_index: u32 = args.0[0].get()?;
    let ty = ctx.wit().stream(type_index as usize);

    let handles = unsafe { ty.new()() };
    let tx_handle = (handles >> 32) as u32;
    let rx_handle = (handles & 0xFFFF_FFFF) as u32;

    let tx = Class::instance(ctx.clone(), StreamWritable::new(type_index, tx_handle))?;
    let rx = make_stream_readable(&ctx, type_index, rx_handle)?;

    let result = rquickjs::Object::new(ctx)?;
    result.set("writable", tx.into_inner())?;
    result.set("readable", rx)?;

    Ok(result.into_value())
}

fn stream_read<'js>(
    this: This<Class<'js, StreamReadable>>,
    ctx: Ctx<'js>,
    args: Rest<Value<'js>>,
) -> rquickjs::Result<Value<'js>> {
    let count: usize = args.0.first().and_then(|v| v.get().ok()).unwrap_or(1);

    let (handle, type_index) = this.0.borrow().end.begin_op()?;

    let (promise, resolve, _reject) = ctx.promise()?;
    let ty = ctx.wit().stream(type_index as usize);

    let buf_size = ty
        .abi_payload_size()
        .checked_mul(count)
        .ok_or_else(|| rquickjs::Error::new_from_js("number", "buffer size overflow"))?;

    let buffer = BufferGuard::new_zeroed(buf_size, ty.abi_payload_align());
    let code = unsafe { ty.read()(handle, buffer.ptr().cast(), count) };
    let mut call = QjsCallContext::default();

    if is_blocked_raw(code) {
        this.0.borrow_mut().end.mark_blocked();
        let pending = Pending::StreamRead {
            call,
            buffer,
            resolve: Persistent::save(&ctx, resolve.into_value()),
            wrapper: Persistent::save(&ctx, this.0.into_inner().into_value()),
        };
        ctx.task().register(handle, pending);
    } else {
        let (actual_count, copy_result) =
            unpack_copy_result(code).expect("non-BLOCKED stream read must decode");

        // Fast path for stream<u8>: return a Uint8Array directly
        let result_val = if matches!(ty.ty(), Some(wit_dylib_ffi::Type::U8)) {
            let vec = unsafe { buffer.into_vec(actual_count as usize) };
            let ta = rquickjs::TypedArray::<u8>::new(ctx.clone(), vec)?;
            ta.into_value()
        } else {
            let arr = rquickjs::Array::new(ctx.clone())?;
            for offset in 0..(actual_count as usize) {
                unsafe { ty.lift(&mut call, buffer.ptr().add(ty.abi_payload_size() * offset)) };
                let val = call.pop_value(&ctx);
                arr.set(offset, val)?;
            }
            drop(buffer);
            arr.into_value()
        };

        this.0.borrow_mut().end.mark_completed(copy_result);

        resolve
            .call::<_, Value>((result_val,))
            .expect("resolve stream read");
    }

    Ok(promise.into_value())
}

fn stream_cancel_read<'js>(
    this: This<Class<'js, StreamReadable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (handle, type_index) = this.0.borrow().end.begin_cancel()?;
    let ty = ctx.wit().stream(type_index as usize);
    let code = unsafe { ty.cancel_read()(handle) };

    match unpack_copy_result(code) {
        None => {
            this.0.borrow_mut().end.mark_cancel_blocked();
            Ok(Value::new_undefined(ctx))
        }
        Some((progress, result)) => {
            this.0.borrow_mut().end.mark_completed(result);
            let obj = Object::new(ctx.clone())?;
            obj.set("progress", progress)?;
            obj.set("result", result as u32)?;
            Ok(obj.into_value())
        }
    }
}

fn stream_drop_readable<'js>(
    this: This<Class<'js, StreamReadable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<()> {
    let mut w = this.0.borrow_mut();

    if let Some(handle) = w.end.handle.take() {
        let ty = ctx.wit().stream(w.end.type_index as usize);
        unsafe { ty.drop_readable()(handle) };
    }

    Ok(())
}

fn stream_write<'js>(
    this: This<Class<'js, StreamWritable>>,
    ctx: Ctx<'js>,
    data: Value<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (handle, type_index) = this.0.borrow().end.begin_op()?;

    let (promise, resolve, _reject) = ctx.promise()?;
    let ty = ctx.wit().stream(type_index as usize);

    let mut call = QjsCallContext::default();
    let (buffer, write_count) = if let Some(arr) = data.as_array() {
        let count = arr.len();
        let buf_size = ty
            .abi_payload_size()
            .checked_mul(count)
            .ok_or_else(|| rquickjs::Error::new_from_js("number", "buffer size overflow"))?;

        let buf = BufferGuard::new_zeroed(buf_size, ty.abi_payload_align());

        for i in 0..count {
            let elem: Value = arr.get(i)?;
            call.push_value(&ctx, elem);
            unsafe { ty.lower(&mut call, buf.ptr().add(ty.abi_payload_size() * i)) };
        }
        (buf, count)
    } else {
        let buf = BufferGuard::new_zeroed(ty.abi_payload_size(), ty.abi_payload_align());
        call.push_value(&ctx, data);
        unsafe { ty.lower(&mut call, buf.ptr()) };
        (buf, 1)
    };

    let code = unsafe { ty.write()(handle, buffer.ptr().cast(), write_count) };

    if is_blocked_raw(code) {
        this.0.borrow_mut().end.mark_blocked();
        let pending = Pending::StreamWrite {
            resolve: Persistent::save(&ctx, resolve.into_value()),
            wrapper: Persistent::save(&ctx, this.0.into_inner().into_value()),
            buffer,
        };
        ctx.task().register(handle, pending);
    } else {
        drop(buffer);
        let (progress, copy_result) = unpack_copy_result(code).expect("non-blocked");
        this.0.borrow_mut().end.mark_completed(copy_result);

        let result = Value::new_number(ctx.clone(), progress as f64);
        resolve
            .call::<_, Value>((result,))
            .expect("resolve stream write");
    }

    Ok(promise.into_value())
}

fn stream_write_all<'js>(
    this: This<Class<'js, StreamWritable>>,
    ctx: Ctx<'js>,
    buffer: Value<'js>,
) -> rquickjs::Result<Value<'js>> {
    let stream_val = this.0.into_inner().into_value();
    write_all_step(ctx, stream_val, buffer, 0)
}

fn write_all_step<'js>(
    ctx: Ctx<'js>,
    stream: Value<'js>,
    buffer: Value<'js>,
    total: usize,
) -> rquickjs::Result<Value<'js>> {
    // Check termination: buffer empty or stream done.
    let state = Class::<StreamWritable>::from_value(&stream)
        .map(|class| class.borrow().end.state)
        .unwrap_or(CopyState::Done);

    let buf_len = if let Some(arr) = buffer.as_array() {
        arr.len()
    } else if let Some(obj) = buffer.as_object() {
        obj.get::<_, usize>("byteLength")
            .or_else(|_| obj.get("length"))
            .unwrap_or(0)
    } else {
        0
    };

    if buf_len == 0 || state == CopyState::Done {
        return Ok(Value::new_number(ctx, total as f64));
    }

    // Call stream.write(buffer) with proper `this` binding.
    let stream_obj = stream
        .as_object()
        .ok_or_else(|| rquickjs::Error::new_from_js("value", "stream object"))?;

    let write_fn: Function = stream_obj.get("write")?;
    let mut call_args = function::Args::new(ctx.clone(), 1);
    call_args.this(stream.clone())?;
    call_args.push_arg(buffer.clone())?;

    let write_result: Value = write_fn.call_arg(call_args)?;

    let promise_obj = write_result
        .as_object()
        .ok_or_else(|| rquickjs::Error::new_from_js("value", "promise"))?;
    let then_fn: Function = promise_obj.get("then")?;

    let stream_c = Cell::new(Some(Persistent::save(&ctx, stream)));
    let buffer_c = Cell::new(Some(Persistent::save(&ctx, buffer)));

    let next = crate::coerce_fn(
        move |ctx: Ctx<'_>, args: Rest<Value<'_>>| -> rquickjs::Result<Value<'_>> {
            let count_val = args
                .0
                .into_iter()
                .next()
                .unwrap_or_else(|| Value::new_undefined(ctx.clone()));

            let count: usize = count_val.get().unwrap_or(0);
            let buf = buffer_c.take().unwrap().restore(&ctx)?;

            let sliced: Value = if let Some(obj) = buf.as_object() {
                let slice_fn: Function = obj.get("slice")?;
                slice_fn.call((count,))?
            } else {
                Value::new_undefined(ctx.clone())
            };
            let s = stream_c.take().unwrap().restore(&ctx)?;
            write_all_step(ctx, s, sliced, total + count)
        },
    );
    let cb = Function::new(ctx.clone(), next)?;
    let mut then_args = function::Args::new(ctx.clone(), 1);
    then_args.this(write_result)?;
    then_args.push_arg(cb)?;
    then_fn.call_arg(then_args)
}

fn stream_cancel_write<'js>(
    this: This<Class<'js, StreamWritable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (handle, type_index) = this.0.borrow().end.begin_cancel()?;
    let ty = ctx.wit().stream(type_index as usize);
    let code = unsafe { ty.cancel_write()(handle) };

    match unpack_copy_result(code) {
        None => {
            this.0.borrow_mut().end.mark_cancel_blocked();
            Ok(Value::new_undefined(ctx))
        }
        Some((progress, result)) => {
            this.0.borrow_mut().end.mark_completed(result);
            let obj = Object::new(ctx.clone())?;
            obj.set("progress", progress)?;
            obj.set("result", result as u32)?;
            Ok(obj.into_value())
        }
    }
}

fn stream_drop_writable<'js>(
    this: This<Class<'js, StreamWritable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<()> {
    let mut w = this.0.borrow_mut();
    if let Some(handle) = w.end.handle.take() {
        let ty = ctx.wit().stream(w.end.type_index as usize);
        unsafe { ty.drop_writable()(handle) };
    }
    Ok(())
}

/// Handle a stream-write completion event in the async callback.
pub(crate) fn handle_write_event(handle: u32, result: u32) {
    let pending = with_ctx(|ctx| ctx.task().take(handle));

    let Pending::StreamWrite {
        resolve, wrapper, ..
    } = pending
    else {
        unreachable!("expected StreamWrite pending");
    };

    let (progress, copy_result) =
        unpack_copy_result(result).expect("StreamWrite callback should not be BLOCKED");

    let result = with_ctx(|ctx| {
        let w = wrapper.restore(ctx).unwrap();
        let cls = Class::<StreamWritable>::from_value(&w).unwrap();

        cls.borrow_mut().end.mark_completed(copy_result);

        let val = Value::new_number(ctx.clone(), progress as f64);
        let res = Persistent::save(ctx, val);
        Some(res)
    });
    resolve_promise(resolve, result);
}

/// Handle a stream-read completion event in the async callback.
pub(crate) fn handle_read_event(handle: u32, result: u32) {
    let pending = with_ctx(|ctx| ctx.task().take(handle));

    let Pending::StreamRead {
        mut call,
        buffer,
        resolve,
        wrapper,
    } = pending
    else {
        unreachable!("expected StreamRead pending");
    };

    let (progress, copy_result) =
        unpack_copy_result(result).expect("StreamRead callback should not be BLOCKED");

    let result = with_ctx(|ctx| {
        let w = wrapper.restore(ctx).unwrap();
        let class = Class::<StreamReadable>::from_value(&w).unwrap();

        let type_index = {
            let mut cls = class.borrow_mut();
            cls.end.mark_completed(copy_result);
            cls.end.type_index
        };

        let ty = ctx.wit().stream(type_index as usize);
        let progress = progress as usize;

        let result_val = if matches!(ty.ty(), Some(wit_dylib_ffi::Type::U8)) {
            let vec = unsafe { buffer.into_vec(progress) };
            let ta = rquickjs::TypedArray::<u8>::new(ctx.clone(), vec).unwrap();
            ta.into_value()
        } else {
            let arr = rquickjs::Array::new(ctx.clone()).unwrap();
            for offset in 0..progress {
                unsafe {
                    let off = ty.abi_payload_size() * offset;
                    ty.lift(&mut call, buffer.ptr().add(off));
                }

                let val = call.pop_value(ctx);
                arr.set(offset, val).unwrap();
            }

            drop(buffer);
            arr.into_value()
        };

        Some(Persistent::save(ctx, result_val))
    });

    resolve_promise(resolve, result);
}
