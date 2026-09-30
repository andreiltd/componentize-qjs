//! WASI component-model stream operations using rquickjs classes.
//!
//! Stream endpoints are represented as native JS classes (`StreamReadable`,
//! `StreamWritable`) whose state lives on the Rust side.  Methods on the
//! shared prototype avoid per-instance closure allocations.
#![allow(unsafe_code)]

mod helpers;

use crate::CtxExt;
use crate::abi::{CopyResult, is_blocked_raw, unpack_copy_result};
use crate::buffer::BufferGuard;
use crate::endpoint::CopyEnd;
use crate::task::Pending;
use crate::typed_array::copy_typed_array_as;
use crate::{QjsCallContext, resolve_promise, symbol_dispose, with_ctx};

use rquickjs::JsLifetime;
use rquickjs::class::{Class, JsClass, Trace};
use rquickjs::function::{self, Opt, This};
use rquickjs::{Ctx, Function, Object, Persistent, Symbol, Value};

const BYTE_ITERATOR_CHUNK_SIZE: usize = 64 * 1024;

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
        helpers::register(ctx, &proto)?;
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
pub(crate) fn make_stream<'js>(ctx: Ctx<'js>, type_index: u32) -> rquickjs::Result<Value<'js>> {
    let ty = ctx
        .wit()
        .iter_streams()
        .nth(type_index as usize)
        .ok_or_else(|| rquickjs::Exception::throw_range(&ctx, "unknown WIT stream type"))?;

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
    let handle = readable.borrow_mut().end.begin_transfer(type_index)?;

    Ok(handle)
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

    match elem_ty {
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
        _ => Ok(None),
    }
}

fn stream_read<'js>(
    this: This<Class<'js, StreamReadable>>,
    ctx: Ctx<'js>,
    count: Opt<usize>,
) -> rquickjs::Result<Value<'js>> {
    stream_read_impl(this, ctx, count.0.unwrap_or(1), false)
}

fn stream_next<'js>(
    this: This<Class<'js, StreamReadable>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (type_index, finished) = {
        let readable = this.0.borrow();
        (readable.end.type_index(), readable.end.is_closed())
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
    ctx.task().ensure_active(&ctx)?;
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
            resolve: Persistent::save(&ctx, resolve),
            wrapper: Persistent::save(&ctx, this.0.into_inner().into_value()),
        };
        ctx.task().register(handle, pending);
    } else {
        let result_val = finish_read(&ctx, &this.0, call, buffer, code, iterator, false)?;

        resolve
            .call::<_, Value>((result_val,))
            .expect("resolve stream read");
    }

    Ok(promise.into_value())
}

/// Finish a stream read from an immediate result, callback, or cancellation.
///
/// Update endpoint ownership before lifting, release closed handles outside the
/// class borrow, and adapt the lifted value to the requested iterator shape.
/// `close` completes an iterator's pending `return()`: drop the readable handle
/// even if the copy was only cancelled, and resolve the read as `done`.
fn finish_read<'js>(
    ctx: &Ctx<'js>,
    cls: &Class<'js, StreamReadable>,
    call: QjsCallContext,
    buffer: BufferGuard,
    code: u32,
    iter: bool,
    close: bool,
) -> rquickjs::Result<Value<'js>> {
    let (progress, result) =
        unpack_copy_result(code).expect("stream read completion must not block");

    let (type_index, dropped_handle) = {
        let mut readable = cls.borrow_mut();
        readable.end.mark_completed(result);

        let handle = if close || result == CopyResult::Dropped {
            readable.end.begin_drop()?
        } else {
            None
        };

        (readable.end.type_index(), handle)
    };

    let ty = ctx.wit().stream(type_index as usize);
    let value = lift_stream_read_result(ctx, ty, call, buffer, progress as usize, result, iter)?;

    if let Some(handle) = dropped_handle {
        unsafe { ty.drop_readable()(handle) };
    }

    if close && iter {
        iterator_result(ctx, Value::new_undefined(ctx.clone()), true)
    } else {
        Ok(value)
    }
}

/// Finish a stream write, committing only the resource groups actually consumed.
///
/// Keep the conversion context alive through settlement in the caller. Host
/// destructors run only after the endpoint's class borrow has been released.
fn finish_write<'js>(
    ctx: &Ctx<'js>,
    cls: &Class<'js, StreamWritable>,
    call: &mut QjsCallContext,
    buffer: BufferGuard,
    code: u32,
) -> rquickjs::Result<Value<'js>> {
    let (progress, result) =
        unpack_copy_result(code).expect("stream write completion must not block");

    drop(buffer);
    call.complete_transfers(progress as usize);

    let (type_index, dropped_handle) = {
        let mut writable = cls.borrow_mut();
        writable.end.mark_completed(result);

        let handle = if result == CopyResult::Dropped {
            writable.end.begin_drop()?
        } else {
            None
        };

        (writable.end.type_index(), handle)
    };

    if let Some(handle) = dropped_handle {
        let ty = ctx.wit().stream(type_index as usize);
        unsafe { ty.drop_writable()(handle) };
    }

    Ok(Value::new_number(ctx.clone(), progress as f64))
}

/// Lift copied elements before releasing their ABI buffer and resource guards.
fn lift_stream_read_result<'js>(
    ctx: &Ctx<'js>,
    ty: wit_dylib_ffi::Stream,
    mut call: QjsCallContext,
    buffer: BufferGuard,
    progress: usize,
    copy_result: CopyResult,
    iter: bool,
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

        if iter {
            if progress == 0 {
                Value::new_undefined(ctx.clone())
            } else {
                arr.get(0)?
            }
        } else {
            arr.into_value()
        }
    };

    if iter {
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
    let (has_handle, pending, cancellable) = {
        let readable = this.0.borrow();
        (
            readable.end.has_handle(),
            readable.end.is_pending(),
            readable.end.can_cancel(),
        )
    };

    if !has_handle {
        return resolved_iterator_result(ctx.clone(), Value::new_undefined(ctx), true);
    }

    if !pending {
        stream_drop_readable(this, ctx.clone())?;
        return resolved_iterator_result(ctx.clone(), Value::new_undefined(ctx), true);
    }

    if !cancellable {
        return Err(rquickjs::Error::new_from_js(
            "stream",
            "iterator return while cancellation is in progress",
        ));
    }

    let (handle, type_index) = this.0.borrow().end.begin_cancel()?;
    let ty = ctx.wit().stream(type_index as usize);
    let (promise, resolve, _reject) = ctx.promise()?;

    ctx.task().unjoin(handle);

    let code = unsafe { ty.cancel_read()(handle) };
    ctx.task()
        .set_stream_iterator_return(handle, Persistent::save(&ctx, resolve));

    if is_blocked_raw(code) {
        ctx.task().rejoin(handle);
        this.0.borrow_mut().end.mark_cancel_blocked();
    } else {
        handle_read_event(handle, code);
    }

    Ok(promise.into_value())
}

pub(crate) fn stream_cancel_read<'js>(
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
    let (handle, type_index) = {
        let mut readable = this.0.borrow_mut();
        (readable.end.begin_drop()?, readable.end.type_index())
    };

    if let Some(handle) = handle {
        let ty = ctx.wit().stream(type_index as usize);
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

fn stream_write_impl<'js>(
    this: This<Class<'js, StreamWritable>>,
    ctx: Ctx<'js>,
    data: Value<'js>,
    mode: StreamWriteMode,
) -> rquickjs::Result<Value<'js>> {
    ctx.task().ensure_active(&ctx)?;
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
            call.transfer_group = i;
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
            resolve: Persistent::save(&ctx, resolve),
            wrapper: Persistent::save(&ctx, this.0.into_inner().into_value()),
            buffer,
        };
        ctx.task().register(handle, pending);
    } else {
        let result = finish_write(&ctx, &this.0, &mut call, buffer, code)?;

        resolve
            .call::<_, Value>((result,))
            .expect("resolve stream write");
    }

    Ok(promise.into_value())
}

pub(crate) fn stream_cancel_write<'js>(
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
    let (handle, type_index) = {
        let mut writable = this.0.borrow_mut();
        (writable.end.begin_drop()?, writable.end.type_index())
    };
    if let Some(handle) = handle {
        let ty = ctx.wit().stream(type_index as usize);
        unsafe { ty.drop_writable()(handle) };
    }
    Ok(())
}

/// Handle a stream-write completion event in the async callback.
pub(crate) fn handle_write_event(handle: u32, result: u32) {
    let pending = with_ctx(|ctx| ctx.task().take(handle));

    let Pending::StreamWrite {
        mut call,
        resolve,
        wrapper,
        buffer,
    } = pending
    else {
        unreachable!("expected StreamWrite pending");
    };

    let result = with_ctx(|ctx| {
        let w = wrapper.restore(ctx).unwrap();
        let class = Class::<StreamWritable>::from_value(&w).unwrap();
        let value = finish_write(ctx, &class, &mut call, buffer, result).unwrap();
        Some(Persistent::save(ctx, value))
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

    let (result, return_result) = with_ctx(|ctx| {
        let w = wrapper.restore(ctx).unwrap();
        let class = Class::<StreamReadable>::from_value(&w).unwrap();

        let close = iterator_return.is_some();
        let result_val = finish_read(ctx, &class, call, buffer, result, iterator, close).unwrap();

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
