//! WASI component-model stream operations using rquickjs classes.
//!
//! Stream endpoints are represented as native JS classes (`StreamReadable`,
//! `StreamWritable`) whose state lives on the Rust side.  Methods on the
//! shared prototype avoid per-instance closure allocations.
#![allow(unsafe_code)]
use crate::CtxExt;
use crate::abi::{CopyEnd, CopyResult, CopyState, is_blocked_raw, unpack_copy_result};
use crate::buffer::BufferGuard;
use crate::task::Pending;
use crate::{QjsCallContext, resolve_promise, symbol_dispose, with_ctx};

use rquickjs::JsLifetime;
use rquickjs::class::{Class, JsClass, Trace};
use rquickjs::function::{self, Rest, This};
use rquickjs::{Ctx, Function, Object, Persistent, Symbol, Value};

use std::cell::Cell;

const BYTE_ITERATOR_CHUNK_SIZE: usize = 64 * 1024;

macro_rules! copy_typed_array_as {
    ($obj:expr, $ty:expr, $t:ty) => {{
        let Some(ta) = $obj.as_typed_array::<$t>() else {
            return Ok(None);
        };

        let slice: &[$t] = ta.as_ref();
        let count = slice.len();

        assert_eq!($ty.abi_payload_size(), std::mem::size_of::<$t>());
        assert!($ty.abi_payload_align() >= std::mem::align_of::<$t>());

        let byte_len = count
            .checked_mul(std::mem::size_of::<$t>())
            .ok_or_else(|| rquickjs::Error::new_from_js("number", "buffer size overflow"))?;

        let buf = BufferGuard::new_zeroed(byte_len, $ty.abi_payload_align());
        if byte_len > 0 {
            unsafe {
                let src = slice.as_ptr() as *const u8;
                let dst = buf.ptr();
                std::ptr::copy_nonoverlapping(src, dst, byte_len);
            }
        }
        Some((buf, count))
    }};
}

macro_rules! typed_array_len_as {
    ($obj:expr, $t:ty) => {
        $obj.as_typed_array::<$t>().map(|array| {
            let slice: &[$t] = array.as_ref();
            slice.len()
        })
    };
}

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
        proto.set("next", Function::new(ctx.clone(), stream_next)?)?;
        proto.set(
            "return",
            Function::new(ctx.clone(), stream_iterator_return)?,
        )?;
        proto.set(
            Symbol::async_iterator(ctx.clone()).as_atom(),
            Function::new(ctx.clone(), stream_async_iterator)?,
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
        proto.set("writeOne", Function::new(ctx.clone(), stream_write_one)?)?;
        proto.set("writeAll", Function::new(ctx.clone(), stream_write_all)?)?;
        proto.set(
            "writeIterableItem",
            Function::new(ctx.clone(), stream_write_iterable_item)?,
        )?;
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

pub(crate) fn lower_iterable<'js>(
    ctx: &Ctx<'js>,
    type_index: u32,
    iterable: Value<'js>,
) -> rquickjs::Result<u32> {
    if !ctx.task().is_active() {
        return Err(rquickjs::Error::new_from_js(
            "async iterable",
            "WIT stream lowering requires an active async call",
        ));
    }

    let wit: Object = ctx.globals().get("wit")?;
    let stream: Function = wit.get("Stream")?;
    let from: Function = stream.get("from")?;
    let pair: Object = from.call((iterable, type_index))?;
    let readable: Value = pair.get("readable")?;
    let readable = Class::<StreamReadable>::from_value(&readable)?;
    let handle = readable
        .borrow_mut()
        .end
        .handle
        .take()
        .ok_or_else(|| rquickjs::Error::new_from_js("stream", "already transferred"))?;

    Ok(handle)
}

fn typed_array_batch_len<'js>(data: &Value<'js>, ty: &wit_dylib_ffi::Stream) -> Option<usize> {
    let obj = data.as_object()?;
    match ty.ty()? {
        wit_dylib_ffi::Type::U8 => typed_array_len_as!(obj, u8),
        wit_dylib_ffi::Type::S8 => typed_array_len_as!(obj, i8),
        wit_dylib_ffi::Type::U16 => typed_array_len_as!(obj, u16),
        wit_dylib_ffi::Type::S16 => typed_array_len_as!(obj, i16),
        wit_dylib_ffi::Type::U32 => typed_array_len_as!(obj, u32),
        wit_dylib_ffi::Type::S32 => typed_array_len_as!(obj, i32),
        wit_dylib_ffi::Type::U64 => typed_array_len_as!(obj, u64),
        wit_dylib_ffi::Type::S64 => typed_array_len_as!(obj, i64),
        wit_dylib_ffi::Type::F32 => typed_array_len_as!(obj, f32),
        wit_dylib_ffi::Type::F64 => typed_array_len_as!(obj, f64),
        _ => None,
    }
}

/// Fast path for `writable.write(typedArray)`
fn try_typed_array_to_buffer<'js>(
    data: &Value<'js>,
    ty: &wit_dylib_ffi::Stream,
) -> rquickjs::Result<Option<(BufferGuard, usize)>> {
    let Some(elem_ty) = ty.ty() else {
        return Ok(None);
    };

    let Some(obj) = data.as_object() else {
        return Ok(None);
    };

    let pair = match elem_ty {
        wit_dylib_ffi::Type::U8 => copy_typed_array_as!(obj, ty, u8),
        wit_dylib_ffi::Type::S8 => copy_typed_array_as!(obj, ty, i8),
        wit_dylib_ffi::Type::U16 => copy_typed_array_as!(obj, ty, u16),
        wit_dylib_ffi::Type::S16 => copy_typed_array_as!(obj, ty, i16),
        wit_dylib_ffi::Type::U32 => copy_typed_array_as!(obj, ty, u32),
        wit_dylib_ffi::Type::S32 => copy_typed_array_as!(obj, ty, i32),
        wit_dylib_ffi::Type::U64 => copy_typed_array_as!(obj, ty, u64),
        wit_dylib_ffi::Type::S64 => copy_typed_array_as!(obj, ty, i64),
        wit_dylib_ffi::Type::F32 => copy_typed_array_as!(obj, ty, f32),
        wit_dylib_ffi::Type::F64 => copy_typed_array_as!(obj, ty, f64),
        _ => return Ok(None),
    };

    Ok(pair)
}

fn stream_read<'js>(
    this: This<Class<'js, StreamReadable>>,
    ctx: Ctx<'js>,
    args: Rest<Value<'js>>,
) -> rquickjs::Result<Value<'js>> {
    let count: usize = args.0.first().and_then(|v| v.get().ok()).unwrap_or(1);
    stream_read_impl(this, ctx, count, false)
}

fn stream_next<'js>(
    this: This<Class<'js, StreamReadable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (type_index, finished) = {
        let readable = this.0.borrow();
        (
            readable.end.type_index,
            readable.end.handle.is_none() || readable.end.state == CopyState::Done,
        )
    };

    if finished {
        return resolved_iterator_result(ctx.clone(), Value::new_undefined(ctx), true);
    }

    let count = if matches!(
        ctx.wit().stream(type_index as usize).ty(),
        Some(wit_dylib_ffi::Type::U8)
    ) {
        BYTE_ITERATOR_CHUNK_SIZE
    } else {
        1
    };

    stream_read_impl(this, ctx, count, true)
}

fn stream_read_impl<'js>(
    this: This<Class<'js, StreamReadable>>,
    ctx: Ctx<'js>,
    count: usize,
    iterator: bool,
) -> rquickjs::Result<Value<'js>> {
    if count == 0 {
        return Err(rquickjs::Error::new_from_js(
            "number",
            "stream read count must be greater than zero",
        ));
    }

    let (handle, type_index) = this.0.borrow().end.begin_op()?;

    let (promise, resolve, _reject) = ctx.promise()?;
    let ty = ctx.wit().stream(type_index as usize);

    let buf_size = ty
        .abi_payload_size()
        .checked_mul(count)
        .ok_or_else(|| rquickjs::Error::new_from_js("number", "buffer size overflow"))?;

    let buffer = BufferGuard::new_zeroed(buf_size, ty.abi_payload_align());
    let code = unsafe { ty.read()(handle, buffer.ptr().cast(), count) };
    let call = QjsCallContext::default();

    if is_blocked_raw(code) {
        this.0.borrow_mut().end.mark_blocked();
        let pending = Pending::StreamRead {
            call,
            buffer,
            iterator,
            iterator_return: None,
            resolve: Persistent::save(&ctx, resolve.into_value()),
            wrapper: Persistent::save(&ctx, this.0.into_inner().into_value()),
        };
        ctx.task().register(handle, pending);
    } else {
        let (actual_count, copy_result) =
            unpack_copy_result(code).expect("non-BLOCKED stream read must decode");

        let dropped_handle = {
            let mut readable = this.0.borrow_mut();
            readable.end.mark_completed(copy_result);
            (copy_result == CopyResult::Dropped)
                .then(|| readable.end.handle.take())
                .flatten()
        };

        let result_val = lift_stream_read_result(
            &ctx,
            ty,
            call,
            buffer,
            actual_count as usize,
            copy_result,
            iterator,
        )?;
        if let Some(handle) = dropped_handle {
            unsafe { ty.drop_readable()(handle) };
        }
        resolve
            .call::<_, Value>((result_val,))
            .expect("resolve stream read");
    }

    Ok(promise.into_value())
}

fn lift_stream_read_result<'js>(
    ctx: &Ctx<'js>,
    ty: wit_dylib_ffi::Stream,
    mut call: QjsCallContext,
    buffer: BufferGuard,
    progress: usize,
    copy_result: CopyResult,
    iterator: bool,
) -> rquickjs::Result<Value<'js>> {
    let value = if matches!(ty.ty(), Some(wit_dylib_ffi::Type::U8)) {
        let vec = unsafe { buffer.into_vec(progress) };
        rquickjs::TypedArray::<u8>::new(ctx.clone(), vec)?.into_value()
    } else {
        let arr = rquickjs::Array::new(ctx.clone())?;
        for offset in 0..progress {
            unsafe { ty.lift(&mut call, buffer.ptr().add(ty.abi_payload_size() * offset)) };
            arr.set(offset, call.pop_value(ctx))?;
        }
        drop(buffer);

        if iterator {
            if progress == 0 {
                Value::new_undefined(ctx.clone())
            } else {
                arr.get(0)?
            }
        } else {
            arr.into_value()
        }
    };

    if iterator {
        iterator_result(
            ctx,
            value,
            progress == 0 && copy_result == CopyResult::Dropped,
        )
    } else {
        Ok(value)
    }
}

fn iterator_result<'js>(
    ctx: &Ctx<'js>,
    value: Value<'js>,
    done: bool,
) -> rquickjs::Result<Value<'js>> {
    let result = Object::new(ctx.clone())?;
    result.set("value", value)?;
    result.set("done", done)?;
    Ok(result.into_value())
}

fn resolved_iterator_result<'js>(
    ctx: Ctx<'js>,
    value: Value<'js>,
    done: bool,
) -> rquickjs::Result<Value<'js>> {
    let (promise, resolve, _reject) = ctx.promise()?;
    let result = iterator_result(&ctx, value, done)?;
    resolve.call::<_, Value>((result,))?;
    Ok(promise.into_value())
}

fn stream_async_iterator<'js>(
    this: This<Class<'js, StreamReadable>>,
) -> rquickjs::Result<Value<'js>> {
    Ok(this.0.into_inner().into_value())
}

fn stream_iterator_return<'js>(
    this: This<Class<'js, StreamReadable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (handle, type_index, state) = {
        let readable = this.0.borrow();
        (
            readable.end.handle,
            readable.end.type_index,
            readable.end.state,
        )
    };

    let Some(handle) = handle else {
        return resolved_iterator_result(ctx.clone(), Value::new_undefined(ctx), true);
    };
    let ty = ctx.wit().stream(type_index as usize);

    if matches!(state, CopyState::Idle | CopyState::Done) {
        this.0.borrow_mut().end.handle.take();
        unsafe { ty.drop_readable()(handle) };
        this.0.borrow_mut().end.state = CopyState::Done;
        return resolved_iterator_result(ctx.clone(), Value::new_undefined(ctx), true);
    }

    if state != CopyState::AsyncCopying {
        return Err(rquickjs::Error::new_from_js(
            "stream",
            "iterator return while cancellation is in progress",
        ));
    }

    let (promise, resolve, _reject) = ctx.promise()?;
    ctx.task().unjoin(handle);
    let code = unsafe { ty.cancel_read()(handle) };
    ctx.task()
        .set_stream_iterator_return(handle, Persistent::save(&ctx, resolve.into_value()));

    if is_blocked_raw(code) {
        ctx.task().rejoin(handle);
        this.0.borrow_mut().end.mark_cancel_blocked();
    } else {
        handle_read_event(handle, code);
    }

    Ok(promise.into_value())
}

fn stream_cancel_read<'js>(
    this: This<Class<'js, StreamReadable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (handle, type_index) = this.0.borrow().end.begin_cancel()?;
    let ty = ctx.wit().stream(type_index as usize);
    ctx.task().unjoin(handle);
    let code = unsafe { ty.cancel_read()(handle) };

    match unpack_copy_result(code) {
        None => {
            ctx.task().rejoin(handle);
            this.0.borrow_mut().end.mark_cancel_blocked();
            Ok(Value::new_undefined(ctx))
        }
        Some((progress, result)) => {
            handle_read_event(handle, code);
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamWriteMode {
    Automatic,
    One,
}

fn stream_write<'js>(
    this: This<Class<'js, StreamWritable>>,
    ctx: Ctx<'js>,
    data: Value<'js>,
) -> rquickjs::Result<Value<'js>> {
    stream_write_impl(this, ctx, data, StreamWriteMode::Automatic)
}

fn stream_write_one<'js>(
    this: This<Class<'js, StreamWritable>>,
    ctx: Ctx<'js>,
    data: Value<'js>,
) -> rquickjs::Result<Value<'js>> {
    stream_write_impl(this, ctx, data, StreamWriteMode::One)
}

fn stream_write_iterable_item<'js>(
    this: This<Class<'js, StreamWritable>>,
    ctx: Ctx<'js>,
    data: Value<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (type_index, closed) = {
        let writable = this.0.borrow();
        (
            writable.end.type_index,
            writable.end.handle.is_none() || writable.end.state == CopyState::Done,
        )
    };
    if closed {
        let result = Value::new_number(ctx.clone(), 0.0);
        return map_write_completion(ctx, result, 1);
    }

    let ty = ctx.wit().stream(type_index as usize);
    let batch_len = typed_array_batch_len(&data, &ty);
    let expected = batch_len.unwrap_or(1);

    let result = if batch_len.is_some() {
        stream_write_all(this, ctx.clone(), data)?
    } else {
        stream_write_one(this, ctx.clone(), data)?
    };

    map_write_completion(ctx, result, expected)
}

fn map_write_completion<'js>(
    ctx: Ctx<'js>,
    result: Value<'js>,
    expected: usize,
) -> rquickjs::Result<Value<'js>> {
    let Some(promise) = result.as_object() else {
        let written: usize = result.get()?;
        let (promise, resolve, _reject) = ctx.promise()?;
        let complete = Value::new_bool(ctx.clone(), written == expected);
        resolve.call::<_, Value>((complete,))?;
        return Ok(promise.into_value());
    };

    let then: Function = promise.get("then")?;
    let complete = crate::coerce_fn(
        move |ctx: Ctx<'_>, args: Rest<Value<'_>>| -> rquickjs::Result<Value<'_>> {
            let written: usize = args
                .0
                .into_iter()
                .next()
                .ok_or_else(|| rquickjs::Error::new_from_js("undefined", "write count"))?
                .get()?;
            Ok(Value::new_bool(ctx, written == expected))
        },
    );
    let callback = Function::new(ctx.clone(), complete)?;
    let mut args = function::Args::new(ctx, 1);
    args.this(result)?;
    args.push_arg(callback)?;
    then.call_arg(args)
}

fn stream_write_impl<'js>(
    this: This<Class<'js, StreamWritable>>,
    ctx: Ctx<'js>,
    data: Value<'js>,
    mode: StreamWriteMode,
) -> rquickjs::Result<Value<'js>> {
    let (handle, type_index) = this.0.borrow().end.begin_op()?;

    let (promise, resolve, _reject) = ctx.promise()?;
    let ty = ctx.wit().stream(type_index as usize);

    let mut call = QjsCallContext::default();
    let typed_array = if mode == StreamWriteMode::One {
        None
    } else {
        try_typed_array_to_buffer(&data, &ty)?
    };

    let (buffer, write_count) = if let Some(pair) = typed_array {
        pair
    } else if mode == StreamWriteMode::Automatic
        && let Some(arr) = data.as_array()
    {
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
            call,
            resolve: Persistent::save(&ctx, resolve.into_value()),
            wrapper: Persistent::save(&ctx, this.0.into_inner().into_value()),
            buffer,
        };
        ctx.task().register(handle, pending);
    } else {
        drop(buffer);
        let (progress, copy_result) = unpack_copy_result(code).expect("non-blocked");
        let dropped_handle = {
            let mut writable = this.0.borrow_mut();
            writable.end.mark_completed(copy_result);
            (copy_result == CopyResult::Dropped)
                .then(|| writable.end.handle.take())
                .flatten()
        };
        if let Some(handle) = dropped_handle {
            unsafe { ty.drop_writable()(handle) };
        }

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
                let mut slice_args = function::Args::new(ctx.clone(), 1);
                slice_args.this(buf.clone())?;
                slice_args.push_arg(count)?;
                slice_fn.call_arg(slice_args)?
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
    ctx.task().unjoin(handle);
    let code = unsafe { ty.cancel_write()(handle) };

    match unpack_copy_result(code) {
        None => {
            ctx.task().rejoin(handle);
            this.0.borrow_mut().end.mark_cancel_blocked();
            Ok(Value::new_undefined(ctx))
        }
        Some((progress, result)) => {
            handle_write_event(handle, code);
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
        call: _call,
        resolve,
        wrapper,
        ..
    } = pending
    else {
        unreachable!("expected StreamWrite pending");
    };

    let (progress, copy_result) =
        unpack_copy_result(result).expect("StreamWrite callback should not be BLOCKED");

    let result = with_ctx(|ctx| {
        let w = wrapper.restore(ctx).unwrap();
        let cls = Class::<StreamWritable>::from_value(&w).unwrap();

        let (type_index, dropped_handle) = {
            let mut writable = cls.borrow_mut();
            writable.end.mark_completed(copy_result);
            (
                writable.end.type_index,
                (copy_result == CopyResult::Dropped)
                    .then(|| writable.end.handle.take())
                    .flatten(),
            )
        };
        if let Some(handle) = dropped_handle {
            let ty = ctx.wit().stream(type_index as usize);
            unsafe { ty.drop_writable()(handle) };
        }

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
        call,
        buffer,
        iterator,
        iterator_return,
        resolve,
        wrapper,
    } = pending
    else {
        unreachable!("expected StreamRead pending");
    };

    let (progress, copy_result) =
        unpack_copy_result(result).expect("StreamRead callback should not be BLOCKED");

    let (result, return_result) = with_ctx(|ctx| {
        let w = wrapper.restore(ctx).unwrap();
        let class = Class::<StreamReadable>::from_value(&w).unwrap();

        let close = iterator_return.is_some();
        let (type_index, dropped_handle) = {
            let mut cls = class.borrow_mut();
            cls.end.mark_completed(copy_result);
            if close {
                cls.end.state = CopyState::Done;
            }
            (
                cls.end.type_index,
                (close || copy_result == CopyResult::Dropped)
                    .then(|| cls.end.handle.take())
                    .flatten(),
            )
        };

        let ty = ctx.wit().stream(type_index as usize);
        let progress = progress as usize;

        let mut result_val =
            lift_stream_read_result(ctx, ty, call, buffer, progress, copy_result, iterator)
                .unwrap();
        if close && iterator {
            result_val = iterator_result(ctx, Value::new_undefined(ctx.clone()), true).unwrap();
        }
        if let Some(handle) = dropped_handle {
            unsafe { ty.drop_readable()(handle) };
        }

        let return_result = iterator_return.map(|resolve| {
            let result = iterator_result(ctx, Value::new_undefined(ctx.clone()), true).unwrap();
            (resolve, Persistent::save(ctx, result))
        });

        (Some(Persistent::save(ctx, result_val)), return_result)
    });

    resolve_promise(resolve, result);
    if let Some((resolve, result)) = return_result {
        resolve_promise(resolve, Some(result));
    }
}
