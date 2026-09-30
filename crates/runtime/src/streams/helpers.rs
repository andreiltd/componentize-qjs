//! Stream convenience algorithms, separate from native ABI reads and writes.

use std::cell::Cell;

use rquickjs::class::Class;
use rquickjs::function::{Args, Rest, This};
use rquickjs::{Ctx, Function, Object, Persistent, Value};

use super::{StreamWritable, stream_write_one};
use crate::CtxExt;
use crate::typed_array::typed_array_len_as;

/// Install the public convenience methods on the writable prototype.
pub(super) fn register<'js>(ctx: &Ctx<'js>, prototype: &Object<'js>) -> rquickjs::Result<()> {
    prototype.set("writeAll", Function::new(ctx.clone(), stream_write_all)?)?;
    prototype.set(
        "writeIterableItem",
        Function::new(ctx.clone(), stream_write_iterable_item)?,
    )
}

/// Write matching typed arrays as batches and all other iterable items as scalars.
///
/// Inspect the endpoint without retaining its borrow across writes, dispatch the
/// appropriate writer, then map its progress to a promise of full completion.
fn stream_write_iterable_item<'js>(
    this: This<Class<'js, StreamWritable>>,
    ctx: Ctx<'js>,
    data: Value<'js>,
) -> rquickjs::Result<Value<'js>> {
    let (type_index, closed) = {
        let writable = this.0.borrow();
        (writable.end.type_index(), writable.end.is_closed())
    };

    if closed {
        let result = Value::new_number(ctx.clone(), 0.0);
        return map_write_completion(ctx, result, 1);
    }

    let ty = ctx.wit().stream(type_index as usize);
    let batch_len = typed_array_batch_len(&data, &ty)?;
    let expected = batch_len.unwrap_or(1);

    let result = if batch_len.is_some() {
        stream_write_all(this, ctx.clone(), data)?
    } else {
        stream_write_one(this, ctx.clone(), data)?
    };

    map_write_completion(ctx, result, expected)
}

/// Map an immediate or promised write count to a promise of full completion.
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
    let mut args = Args::new(ctx, 1);

    args.this(result)?;
    args.push_arg(callback)?;
    then.call_arg(args)
}

/// Write an entire buffer, preserving immediate completion for empty or closed writes.
fn stream_write_all<'js>(
    this: This<Class<'js, StreamWritable>>,
    ctx: Ctx<'js>,
    buffer: Value<'js>,
) -> rquickjs::Result<Value<'js>> {
    let stream = this.0.into_inner().into_value();
    write_all_step(ctx, stream, buffer, 0)
}

/// Write a suffix, retain its JS values, and continue after the promise settles.
///
/// Validate progress before slicing, stop on closure, and consume captured values
/// only once even when a custom writer supplies its own thenable.
fn write_all_step<'js>(
    ctx: Ctx<'js>,
    stream: Value<'js>,
    buffer: Value<'js>,
    total: usize,
) -> rquickjs::Result<Value<'js>> {
    let class = Class::<StreamWritable>::from_value(&stream)?;
    let closed = class.borrow().end.is_closed();

    if closed {
        return Ok(Value::new_number(ctx, total as f64));
    }

    let buffer_len = write_all_buffer_len(&buffer)?;

    if buffer_len == 0 {
        return Ok(Value::new_number(ctx, total as f64));
    }

    let stream_obj = stream
        .as_object()
        .ok_or_else(|| rquickjs::Error::new_from_js("value", "stream object"))?;

    let write_fn: Function = stream_obj.get("write")?;
    let mut call_args = Args::new(ctx.clone(), 1);
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
                .ok_or_else(|| rquickjs::Error::new_from_js("undefined", "write count"))?;
            let count: usize = count_val.get()?;
            let buf = buffer_c
                .take()
                .ok_or_else(|| rquickjs::Error::new_from_js("undefined", "write buffer"))?
                .restore(&ctx)?;
            let stream = stream_c
                .take()
                .ok_or_else(|| rquickjs::Error::new_from_js("undefined", "stream"))?
                .restore(&ctx)?;

            let buffer_len = write_all_buffer_len(&buf)?;

            if count > buffer_len {
                return Err(rquickjs::Exception::throw_range(
                    &ctx,
                    &format!("stream write reported {count} items for a {buffer_len}-item buffer"),
                ));
            }

            let class = Class::<StreamWritable>::from_value(&stream)?;
            let closed = class.borrow().end.is_closed();

            if count == 0 {
                if closed {
                    return Ok(Value::new_number(ctx, total as f64));
                }

                return Err(rquickjs::Exception::throw_range(
                    &ctx,
                    "stream write made no progress",
                ));
            }

            if closed {
                return Ok(Value::new_number(ctx, (total + count) as f64));
            }

            let obj = buf
                .as_object()
                .ok_or_else(|| rquickjs::Error::new_from_js(buf.type_of().as_str(), "array"))?;

            let slice_fn: Function = obj.get("slice")?;
            let mut slice_args = Args::new(ctx.clone(), 1);

            slice_args.this(buf.clone())?;
            slice_args.push_arg(count)?;
            let sliced = slice_fn.call_arg(slice_args)?;

            write_all_step(ctx, stream, sliced, total + count)
        },
    );

    let callback = Function::new(ctx.clone(), next)?;
    let mut then_args = Args::new(ctx.clone(), 1);

    then_args.this(write_result)?;
    then_args.push_arg(callback)?;
    then_fn.call_arg(then_args)
}

/// Recognize typed arrays whose element type matches the stream payload.
fn typed_array_batch_len<'js>(
    data: &Value<'js>,
    ty: &wit_dylib_ffi::Stream,
) -> rquickjs::Result<Option<usize>> {
    let Some((obj, elem_ty)) = data.as_object().zip(ty.ty()) else {
        return Ok(None);
    };

    match elem_ty {
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
        _ => Ok(None),
    }
}

/// Preserve native array lengths and checked numeric conversion for writeAll.
fn write_all_buffer_len(value: &Value<'_>) -> rquickjs::Result<usize> {
    if let Some(array) = value.as_array() {
        return Ok(array.len());
    }

    let object = value
        .as_object()
        .ok_or_else(|| rquickjs::Error::new_from_js(value.type_of().as_str(), "array"))?;

    object.get("length")
}
