//! `Call` trait implementation for quickjs to/from wit type conversions.
use crate::CtxExt;
use crate::futures::{FutureReadable, FutureWritable};
use crate::resources::{exported_resource_to_handle, imported_resource_to_handle};
use crate::streams::{StreamReadable, StreamWritable};
use crate::trivia::fn_lookup;
use crate::{BorrowedResource, QjsCallContext, with_ctx};

use rquickjs::class::Class;
use rquickjs::{IntoJs, Persistent, Value};
use smallvec::SmallVec;
use wit_dylib_ffi::{
    Call, Enum, Flags, Future, List, Record, Resource, Stream, Tuple, Type, Variant, WitOption,
    WitResult,
};

use std::alloc::Layout;

/// Pop a value from the stack, restore it in the current JS context, and transform it.
fn pop_with<R: 'static>(cx: &mut QjsCallContext, f: impl FnOnce(Value<'_>) -> R) -> R {
    let persistent = cx.pop_persistent();
    with_ctx(|ctx| {
        let v = persistent.restore(ctx).unwrap();
        f(v)
    })
}

/// Create a JS value in the current context and push it onto the stack.
fn push_with(cx: &mut QjsCallContext, f: impl for<'js> FnOnce(&rquickjs::Ctx<'js>) -> Value<'js>) {
    with_ctx(|ctx| {
        let v = f(ctx);
        cx.push_value(ctx, v);
    });
}

/// Extract a TypedArray<T> and memcpy its bytes into a new buffer.
/// This has to be macro because TypedArrayItem trait is not public.
macro_rules! try_typed_array_copy {
    ($val:expr, $t:ty) => {
        $val.as_object()
            .and_then(|o| o.as_typed_array::<$t>())
            .map(|ta| {
                let slice: &[$t] = ta.as_ref();
                let count = slice.len();
                let byte_len = count * std::mem::size_of::<$t>();
                let layout = Layout::from_size_align(byte_len, std::mem::align_of::<$t>()).unwrap();
                let buf = if byte_len == 0 {
                    std::ptr::NonNull::<u8>::dangling().as_ptr()
                } else {
                    let buf = unsafe { std::alloc::alloc(layout) };
                    let ptr = slice.as_ptr() as *const u8;
                    unsafe { std::ptr::copy_nonoverlapping(ptr, buf, byte_len) };
                    buf
                };
                (buf as *const u8, count, layout)
            })
    };
}

impl Call for QjsCallContext {
    unsafe fn defer_deallocate(&mut self, ptr: *mut u8, layout: Layout) {
        self.deferred_deallocs.push((ptr, layout));
    }

    fn pop_bool(&mut self) -> bool {
        pop_with(self, |v| v.as_bool().expect("expected bool"))
    }

    fn pop_u8(&mut self) -> u8 {
        pop_with(self, |v| v.get::<i32>().expect("expected number") as u8)
    }

    fn pop_s8(&mut self) -> i8 {
        pop_with(self, |v| v.get::<i32>().expect("expected number") as i8)
    }

    fn pop_u16(&mut self) -> u16 {
        pop_with(self, |v| v.get::<i32>().expect("expected number") as u16)
    }

    fn pop_s16(&mut self) -> i16 {
        pop_with(self, |v| v.get::<i32>().expect("expected number") as i16)
    }

    fn pop_u32(&mut self) -> u32 {
        pop_with(self, |v| v.get::<i32>().expect("expected number") as u32)
    }

    fn pop_s32(&mut self) -> i32 {
        pop_with(self, |v| v.get().expect("expected number"))
    }

    fn pop_u64(&mut self) -> u64 {
        pop_with(self, |v| v.get().expect("expected number"))
    }

    fn pop_s64(&mut self) -> i64 {
        pop_with(self, |v| v.get().expect("expected number"))
    }

    fn pop_f32(&mut self) -> f32 {
        pop_with(self, |v| v.get::<f64>().expect("expected number") as f32)
    }

    fn pop_f64(&mut self) -> f64 {
        pop_with(self, |v| v.get().expect("expected number"))
    }

    fn pop_char(&mut self) -> char {
        pop_with(self, |v| {
            let s = v.get::<String>().expect("expected string");
            let mut chars = s.chars();
            let c = chars.next().expect("expected non-empty string for char");
            if chars.next().is_some() {
                panic!("expected single char, got {} chars", s.chars().count());
            }
            c
        })
    }

    fn pop_string(&mut self) -> &str {
        let persistent = self.stack.pop().expect("stack underflow");
        let s = with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            val.get::<String>().expect("expected string")
        });

        self.temp_strings.push(s);
        self.temp_strings.last().unwrap()
    }

    unsafe fn maybe_pop_list(&mut self, ty: List) -> Option<(*const u8, usize)> {
        let persistent = self.stack.last()?.clone();

        let result = with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            match ty.ty() {
                Type::U8 => try_typed_array_copy!(val, u8),
                Type::S8 => try_typed_array_copy!(val, i8),
                Type::U16 => try_typed_array_copy!(val, u16),
                Type::S16 => try_typed_array_copy!(val, i16),
                Type::U32 => try_typed_array_copy!(val, u32),
                Type::S32 => try_typed_array_copy!(val, i32),
                Type::U64 => try_typed_array_copy!(val, u64),
                Type::S64 => try_typed_array_copy!(val, i64),
                Type::F32 => try_typed_array_copy!(val, f32),
                Type::F64 => try_typed_array_copy!(val, f64),
                _ => None,
            }
        });

        result.map(|(ptr, count, layout)| {
            if layout.size() > 0 {
                self.deferred_deallocs.push((ptr as *mut u8, layout));
            }
            self.stack.pop();
            (ptr, count)
        })
    }

    fn pop_list(&mut self, _ty: List) -> usize {
        self.iter_stack.push(0);
        let persistent = self.stack.last().expect("stack underflow").clone();

        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let arr = val.as_array().expect("expected array");
            arr.len()
        })
    }

    fn pop_iter_next(&mut self, _ty: List) {
        let index = *self.iter_stack.last().expect("iter_stack underflow");
        let arr_persistent = self.stack.last().expect("stack underflow").clone();

        with_ctx(|ctx| {
            let arr_val = arr_persistent.restore(ctx).unwrap();
            let arr = arr_val.as_array().expect("expected array");
            let elem: Value = arr.get(index).unwrap();
            self.stack.push(Persistent::save(ctx, elem));
        });

        *self.iter_stack.last_mut().unwrap() = index + 1;
    }

    fn pop_iter(&mut self, _ty: List) {
        self.iter_stack.pop().expect("iter_stack underflow");
        self.stack.pop().expect("stack underflow");
    }

    fn pop_option(&mut self, _ty: WitOption) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");

        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            if val.is_null() || val.is_undefined() {
                0
            } else {
                self.stack.push(Persistent::save(ctx, val));
                1
            }
        })
    }

    fn pop_result(&mut self, ty: WitResult) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let obj = val.as_object().expect("expected object");
            let tag: String = obj.get("tag").expect("expected tag");

            let is_err = tag != "ok";
            let discriminant = if is_err { 1u32 } else { 0u32 };
            let has_payload = if is_err {
                ty.err().is_some()
            } else {
                ty.ok().is_some()
            };

            if has_payload {
                let inner: Value = obj.get("val").unwrap_or(Value::new_undefined(ctx.clone()));
                self.stack.push(Persistent::save(ctx, inner));
            }

            discriminant
        })
    }

    fn pop_variant(&mut self, ty: Variant) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let obj = val.as_object().expect("expected object");
            let tag: u32 = obj.get("tag").expect("expected tag");

            let has_payload = ty
                .cases()
                .nth(tag as usize)
                .map(|(_, case_ty)| case_ty.is_some())
                .unwrap_or(false);

            if has_payload {
                let inner: Value = obj.get("val").unwrap_or(Value::new_undefined(ctx.clone()));
                self.stack.push(Persistent::save(ctx, inner));
            }
            tag
        })
    }

    fn pop_enum(&mut self, _ty: Enum) -> u32 {
        pop_with(self, |v| v.get().expect("expected number"))
    }

    fn pop_flags(&mut self, _ty: Flags) -> u32 {
        pop_with(self, |v| v.get().expect("expected number"))
    }

    fn pop_borrow(&mut self, ty: Resource) -> u32 {
        let persistent = self.pop_persistent();
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            if ty.new().is_some() {
                exported_resource_to_handle(ctx, ty, &val)
            } else {
                imported_resource_to_handle(&val)
            }
        })
    }

    fn pop_own(&mut self, ty: Resource) -> u32 {
        let persistent = self.pop_persistent();
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            if ty.new().is_some() {
                exported_resource_to_handle(ctx, ty, &val)
            } else {
                imported_resource_to_handle(&val)
            }
        })
    }

    fn pop_tuple(&mut self, ty: Tuple) {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let arr = val.as_array().expect("expected array");
            for i in (0..ty.types().len()).rev() {
                let elem: Value = arr.get(i).unwrap();
                self.stack.push(Persistent::save(ctx, elem));
            }
        });
    }

    fn pop_record(&mut self, ty: Record) {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let obj = val.as_object().expect("expected object");
            for (name, _) in ty.fields().rev() {
                let field: Value = obj.get(fn_lookup(ctx, name)).unwrap();
                self.stack.push(Persistent::save(ctx, field));
            }
        });
    }

    fn pop_future(&mut self, _ty: Future) -> u32 {
        pop_with(self, |v| {
            if let Ok(class) = Class::<FutureReadable>::from_value(&v) {
                return class.borrow().end.handle.expect("future already dropped");
            }
            if let Ok(class) = Class::<FutureWritable>::from_value(&v) {
                return class.borrow().end.handle.expect("future already dropped");
            }
            v.get().expect("expected future handle")
        })
    }

    fn pop_stream(&mut self, _ty: Stream) -> u32 {
        pop_with(self, |v| {
            if let Ok(class) = Class::<StreamReadable>::from_value(&v) {
                return class.borrow().end.handle.expect("stream already dropped");
            }
            if let Ok(class) = Class::<StreamWritable>::from_value(&v) {
                return class.borrow().end.handle.expect("stream already dropped");
            }
            v.get().expect("expected stream handle")
        })
    }

    // Push operations
    fn push_bool(&mut self, val: bool) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_u8(&mut self, val: u8) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_s8(&mut self, val: i8) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_u16(&mut self, val: u16) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_s16(&mut self, val: i16) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_u32(&mut self, val: u32) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_s32(&mut self, val: i32) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_u64(&mut self, val: u64) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_s64(&mut self, val: i64) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_f32(&mut self, val: f32) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_f64(&mut self, val: f64) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_char(&mut self, val: char) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_string(&mut self, val: String) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    unsafe fn push_raw_list(&mut self, ty: List, ptr: *mut u8, len: usize) -> bool {
        if !matches!(ty.ty(), Type::U8 | Type::S8) {
            return false;
        }

        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };

        with_ctx(|ctx| {
            let ta = rquickjs::TypedArray::<u8>::new(ctx.clone(), slice).unwrap();
            self.stack.push(Persistent::save(ctx, ta.into_value()));
        });
        true
    }

    fn push_list(&mut self, _ty: List, _capacity: usize) {
        push_with(self, |ctx| {
            rquickjs::Array::new(ctx.clone()).unwrap().into_value()
        });
    }

    fn list_append(&mut self, _ty: List) {
        let elem = self.stack.pop().expect("stack underflow");
        let arr_persistent = self.stack.last().expect("stack underflow").clone();

        with_ctx(|ctx| {
            let arr_val = arr_persistent.restore(ctx).unwrap();
            let arr = arr_val.as_array().expect("expected array");
            let val = elem.restore(ctx).unwrap();
            let len = arr.len();
            arr.set(len, val).unwrap();
        });
    }

    fn push_option(&mut self, _ty: WitOption, is_some: bool) {
        if !is_some {
            push_with(self, |ctx| Value::new_null(ctx.clone()));
        }
    }

    fn push_result(&mut self, ty: WitResult, is_err: bool) {
        let has_payload = if is_err {
            ty.err().is_some()
        } else {
            ty.ok().is_some()
        };

        let inner = if has_payload { self.stack.pop() } else { None };

        with_ctx(|ctx| {
            let obj = rquickjs::Object::new(ctx.clone()).unwrap();
            obj.set("tag", if is_err { "err" } else { "ok" }).unwrap();
            if let Some(val) = inner {
                obj.set("val", val.restore(ctx).unwrap()).unwrap();
            }
            self.stack.push(Persistent::save(ctx, obj.into_value()));
        });
    }

    fn push_variant(&mut self, ty: Variant, tag: u32) {
        let has_payload = ty
            .cases()
            .nth(tag as usize)
            .map(|(_, ty)| ty.is_some())
            .unwrap_or(false);

        let inner = if has_payload { self.stack.pop() } else { None };

        with_ctx(|ctx| {
            let obj = rquickjs::Object::new(ctx.clone()).unwrap();
            obj.set("tag", tag).unwrap();
            if let Some(val) = inner {
                obj.set("val", val.restore(ctx).unwrap()).unwrap();
            }
            self.stack.push(Persistent::save(ctx, obj.into_value()));
        });
    }

    fn push_enum(&mut self, _ty: Enum, val: u32) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_flags(&mut self, _ty: Flags, val: u32) {
        push_with(self, |ctx| val.into_js(ctx).unwrap());
    }

    fn push_borrow(&mut self, ty: Resource, handle: u32) {
        with_ctx(|ctx| {
            let val = if ty.rep().is_some() {
                let rep = handle as usize;
                ctx.resources().get(rep).restore(ctx).unwrap()
            } else {
                self.borrows.push(BorrowedResource {
                    handle,
                    drop_fn: ty.drop(),
                });

                let obj = rquickjs::Object::new(ctx.clone()).unwrap();
                obj.set("componentize_js_handle", handle).unwrap();
                obj.into_value()
            };
            self.push_value(ctx, val);
        });
    }

    fn push_own(&mut self, ty: Resource, handle: u32) {
        with_ctx(|ctx| {
            let val = if ty.rep().is_some() {
                let persistent = ctx.resources().remove(handle as usize);
                let val = persistent.restore(ctx).unwrap();
                if let Some(obj) = val.as_object() {
                    let _ = obj.remove("componentize_js_handle");
                }
                val
            } else {
                let obj = rquickjs::Object::new(ctx.clone()).unwrap();
                obj.set("componentize_js_handle", handle).unwrap();
                obj.into_value()
            };
            self.push_value(ctx, val);
        });
    }

    fn push_tuple(&mut self, ty: Tuple) {
        let len = ty.types().len();
        let elems: SmallVec<[_; 8]> = (0..len)
            .map(|_| self.stack.pop().expect("stack underflow"))
            .collect();

        with_ctx(|ctx| {
            let arr = rquickjs::Array::new(ctx.clone()).unwrap();
            for (i, elem) in elems.into_iter().rev().enumerate() {
                arr.set(i, elem.restore(ctx).unwrap()).unwrap();
            }
            self.stack.push(Persistent::save(ctx, arr.into_value()));
        });
    }

    fn push_record(&mut self, ty: Record) {
        let fields: SmallVec<[_; 16]> = ty.fields().collect();
        let vals: SmallVec<[_; 16]> = (0..fields.len())
            .map(|_| self.stack.pop().expect("stack underflow"))
            .collect();

        with_ctx(|ctx| {
            let obj = rquickjs::Object::new(ctx.clone()).unwrap();
            for ((name, _), val) in fields.iter().zip(vals.into_iter().rev()) {
                obj.set(fn_lookup(ctx, name), val.restore(ctx).unwrap())
                    .unwrap();
            }
            self.stack.push(Persistent::save(ctx, obj.into_value()));
        });
    }

    fn push_future(&mut self, ty: Future, handle: u32) {
        with_ctx(|ctx| {
            let type_index =
                ctx.wit()
                    .iter_futures()
                    .position(|f| f.ty() == ty.ty())
                    .expect("matching future type must exist in WIT") as u32;

            let obj = crate::futures::make_future_readable(ctx, type_index, handle).unwrap();
            self.stack.push(Persistent::save(ctx, obj.into_value()));
        });
    }

    fn push_stream(&mut self, ty: Stream, handle: u32) {
        with_ctx(|ctx| {
            let type_index =
                ctx.wit()
                    .iter_streams()
                    .position(|s| s.ty() == ty.ty())
                    .expect("matching stream type must exist in WIT") as u32;

            let obj = crate::streams::make_stream_readable(ctx, type_index, handle).unwrap();
            self.stack.push(Persistent::save(ctx, obj.into_value()));
        });
    }
}
