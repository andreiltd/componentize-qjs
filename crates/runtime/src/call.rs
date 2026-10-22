//! Canonical call state, resource lifetimes, and QuickJS/WIT conversions.

use crate::CtxExt;
use crate::futures::{FutureReadable, FutureWritable};
use crate::resources::{
    BorrowedResource, ImportedResourceLoan, ImportedResourceTransfer, borrow_imported_resource,
    exported_resource_to_handle, make_imported_borrowed, make_imported_owned,
    transfer_imported_resource,
};
use crate::streams::{StreamReadable, StreamWritable};
use crate::tagged::decode_tagged;
use crate::trivia::fn_lookup;
use crate::typed_array::try_typed_array_copy;
use crate::with_ctx;

use rquickjs::class::Class;
use rquickjs::function::This;
use rquickjs::{
    BigInt, CatchResultExt, Coerced, Constructor, Function, IntoJs, Persistent, Symbol, Value,
};
use smallvec::SmallVec;
use wit_dylib_ffi::{
    Call, Enum, Flags, Future, List, Map, Record, Resource, Stream, Tuple, Type, Variant,
    WitOption, WitResult,
};

use std::alloc::Layout;

/// Owns conversion values and resource guards for one import or export call.
#[derive(Default)]
pub struct QjsCallContext {
    /// Value stack for WIT to JS: arguments in, result out.
    stack: Vec<Persistent<Value<'static>>>,
    /// Tracks the current index per nested list iteration.
    iter_stack: SmallVec<[usize; 4]>,
    /// Keeps borrowed `&str` returns alive across FFI boundaries.
    temp_strings: SmallVec<[String; 4]>,
    /// Raw allocations to free when this context is dropped.
    deferred_deallocs: SmallVec<[(*mut u8, Layout); 4]>,
    /// Keeps imported resources alive while an outgoing call borrows them.
    loans: SmallVec<[ImportedResourceLoan; 4]>,
    /// Own transfers grouped by stream element, or group zero for ordinary calls.
    transfers: SmallVec<[(usize, ImportedResourceTransfer); 4]>,
    /// Group assigned to subsequently lowered owned resources.
    transfer_group: usize,
    /// Imported resource borrows to drop when this context is dropped.
    borrows: SmallVec<[BorrowedResource; 4]>,
}

impl QjsCallContext {
    /// Commit consumed groups and roll back unconsumed ownership transfers.
    pub(crate) fn complete_transfers(&mut self, consumed_groups: usize) {
        for (group, transfer) in self.transfers.drain(..) {
            if group < consumed_groups {
                transfer.commit();
            }
        }
    }

    /// Push a stream element, grouping its owned transfers by element index.
    pub(crate) fn push_stream_element<'js>(
        &mut self,
        ctx: &rquickjs::Ctx<'js>,
        value: Value<'js>,
        index: usize,
    ) {
        self.transfer_group = index;
        self.push_value(ctx, value);
    }

    /// Retain a JS value on the conversion stack.
    pub(crate) fn push_value<'js>(&mut self, ctx: &rquickjs::Ctx<'js>, val: Value<'js>) {
        self.stack.push(Persistent::save(ctx, val));
    }

    /// Restore and remove the top conversion value.
    pub(crate) fn pop_value<'js>(&mut self, ctx: &rquickjs::Ctx<'js>) -> Value<'js> {
        self.pop_persistent().restore(ctx).expect("stack underflow")
    }

    /// Drain canonical arguments in call order, retaining storage for the result.
    pub(crate) fn drain_values(
        &mut self,
    ) -> impl ExactSizeIterator<Item = Persistent<Value<'static>>> {
        self.stack.drain(..)
    }

    /// Take canonical arguments in call order without allocating another vector.
    ///
    /// The iterator does not borrow this context, so async dispatch can retain its
    /// resource guards in the active task before looking up JavaScript exports.
    pub(crate) fn take_values(&mut self) -> std::vec::IntoIter<Persistent<Value<'static>>> {
        std::mem::take(&mut self.stack).into_iter()
    }

    /// Remove the top persistent value, requiring a nonempty stack.
    fn pop_persistent(&mut self) -> Persistent<Value<'static>> {
        self.stack.pop().expect("stack underflow")
    }

    /// Remove the top persistent value if one is present.
    fn maybe_pop_persistent(&mut self) -> Option<Persistent<Value<'static>>> {
        self.stack.pop()
    }

    /// Restore and remove an optional conversion result.
    pub(crate) fn maybe_pop_value<'js>(
        &mut self,
        ctx: &rquickjs::Ctx<'js>,
    ) -> rquickjs::Result<Option<Value<'js>>> {
        self.maybe_pop_persistent()
            .map(|persistent| persistent.restore(ctx))
            .transpose()
    }

    /// Pop a value, restore it in the active JS context, and transform it.
    fn pop_with<R: 'static>(&mut self, f: impl FnOnce(Value<'_>) -> R) -> R {
        let persistent = self.pop_persistent();

        with_ctx(|ctx| {
            let v = persistent.restore(ctx).unwrap();
            f(v)
        })
    }

    /// Create a value in the active JS context and retain it on the stack.
    fn push_with(&mut self, f: impl for<'js> FnOnce(&rquickjs::Ctx<'js>) -> Value<'js>) {
        with_ctx(|ctx| {
            let v = f(ctx);
            self.push_value(ctx, v);
        });
    }
}

impl Drop for QjsCallContext {
    /// Roll back transfers and free buffers before releasing loans and borrows.
    fn drop(&mut self) {
        self.transfers.clear();

        for (ptr, layout) in self.deferred_deallocs.drain(..) {
            if layout.size() > 0 {
                unsafe {
                    std::alloc::dealloc(ptr, layout);
                }
            }
        }

        self.loans.clear();
        self.borrows.clear();
    }
}

/// Whether an option's payload is itself an option (after resolving aliases).
fn option_is_nested(ty: WitOption) -> bool {
    let mut inner = ty.ty();

    loop {
        match inner {
            Type::Option(_) => return true,
            Type::Alias(alias) => inner = alias.ty(),
            _ => return false,
        }
    }
}

impl Call for QjsCallContext {
    unsafe fn defer_deallocate(&mut self, ptr: *mut u8, layout: Layout) {
        self.deferred_deallocs.push((ptr, layout));
    }

    fn pop_bool(&mut self) -> bool {
        self.pop_with(|v| v.as_bool().expect("expected bool"))
    }

    fn pop_u8(&mut self) -> u8 {
        self.pop_with(|v| v.get::<i32>().expect("expected number") as u8)
    }

    fn pop_s8(&mut self) -> i8 {
        self.pop_with(|v| v.get::<i32>().expect("expected number") as i8)
    }

    fn pop_u16(&mut self) -> u16 {
        self.pop_with(|v| v.get::<i32>().expect("expected number") as u16)
    }

    fn pop_s16(&mut self) -> i16 {
        self.pop_with(|v| v.get::<i32>().expect("expected number") as i16)
    }

    fn pop_u32(&mut self) -> u32 {
        self.pop_with(|v| v.get::<i32>().expect("expected number") as u32)
    }

    fn pop_s32(&mut self) -> i32 {
        self.pop_with(|v| v.get().expect("expected number"))
    }

    fn pop_u64(&mut self) -> u64 {
        self.pop_with(|v| {
            assert!(v.is_big_int(), "expected bigint");
            let mut result = 0;
            let status = unsafe {
                rquickjs::qjs::JS_ToBigUint64(v.ctx().as_raw().as_ptr(), &mut result, v.as_raw())
            };
            assert_eq!(status, 0, "failed to read bigint as u64");
            result
        })
    }

    fn pop_s64(&mut self) -> i64 {
        self.pop_with(|v| {
            assert!(v.is_big_int(), "expected bigint");
            let mut result = 0;
            let status = unsafe {
                rquickjs::qjs::JS_ToBigInt64(v.ctx().as_raw().as_ptr(), &mut result, v.as_raw())
            };
            assert_eq!(status, 0, "failed to read bigint as s64");
            result
        })
    }

    fn pop_f32(&mut self) -> f32 {
        self.pop_with(|v| v.get::<f64>().expect("expected number") as f32)
    }

    fn pop_f64(&mut self) -> f64 {
        self.pop_with(|v| v.get().expect("expected number"))
    }

    fn pop_char(&mut self) -> char {
        self.pop_with(|v| {
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
            let result = match ty.ty() {
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
                _ => Ok(None),
            };
            result.catch(ctx).expect("Failed to copy typed array")
        });

        result.map(|(buffer, count)| {
            let (ptr, layout) = buffer.into_raw();
            if layout.size() > 0 {
                self.deferred_deallocs.push((ptr, layout));
            }
            self.stack.pop();
            (ptr.cast_const(), count)
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

    fn pop_list_iter_next(&mut self, _ty: List) {
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

    fn pop_list_iter(&mut self, _ty: List) {
        self.iter_stack.pop().expect("iter_stack underflow");
        self.stack.pop().expect("stack underflow");
    }

    fn pop_map(&mut self, _ty: Map) -> usize {
        let persistent = self.stack.pop().expect("stack underflow");

        with_ctx(|ctx| {
            let value = persistent.restore(ctx).unwrap();
            let map = value.as_object().expect("expected Map");
            let constructor: Constructor = ctx.globals().get("Map").expect("Map not found");
            assert!(map.is_instance_of(&constructor), "expected Map");

            let len = map.get("size").expect("expected Map size");
            let entries: Function = map.get("entries").expect("Map.entries not found");
            let iterator: Value = entries
                .call((This(map.clone()),))
                .expect("failed to create Map iterator");
            self.stack.push(Persistent::save(ctx, iterator));
            len
        })
    }

    fn pop_map_iter_next(&mut self, _ty: Map) {
        let persistent = self.stack.last().expect("stack underflow").clone();

        with_ctx(|ctx| {
            let value = persistent.restore(ctx).unwrap();
            let iterator = value.as_object().expect("expected Map iterator");
            let next: Function = iterator.get("next").expect("Map iterator.next not found");
            let result: rquickjs::Object = next
                .call((This(iterator.clone()),))
                .expect("failed to advance Map iterator");
            let entry: rquickjs::Array = result.get("value").expect("expected Map entry");
            let key: Value = entry.get(0).expect("expected Map key");
            let value: Value = entry.get(1).expect("expected Map value");

            self.stack.push(Persistent::save(ctx, value));
            self.stack.push(Persistent::save(ctx, key));
        });
    }

    fn pop_map_iter(&mut self, _ty: Map) {
        self.stack.pop().expect("stack underflow");
    }

    fn pop_option(&mut self, ty: WitOption) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");

        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();

            if option_is_nested(ty) {
                // Nested option: { tag: "some", val } | { tag: "none" }.
                let (discriminant, payload) =
                    decode_tagged(ctx, val, "nested option", [("none", false), ("some", true)])
                        .catch(ctx)
                        .expect("invalid nested option");
                if let Some(inner) = payload {
                    self.stack.push(Persistent::save(ctx, inner));
                }
                u32::try_from(discriminant).expect("nested option discriminant overflow")
            } else if val.is_null() || val.is_undefined() {
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
            let (discriminant, payload) = decode_tagged(
                ctx,
                val,
                "result",
                [("ok", ty.ok().is_some()), ("err", ty.err().is_some())],
            )
            .catch(ctx)
            .expect("invalid result");
            if let Some(inner) = payload {
                self.stack.push(Persistent::save(ctx, inner));
            }
            u32::try_from(discriminant).expect("result discriminant overflow")
        })
    }

    fn pop_variant(&mut self, ty: Variant) -> u32 {
        let persistent = self.stack.pop().expect("stack underflow");
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let cases = ty
                .cases()
                .map(|(name, payload_ty)| (name, payload_ty.is_some()));

            let (discriminant, payload) = decode_tagged(ctx, val, "variant", cases)
                .catch(ctx)
                .expect("invalid variant");

            if let Some(inner) = payload {
                self.stack.push(Persistent::save(ctx, inner));
            }
            u32::try_from(discriminant).expect("variant discriminant overflow")
        })
    }

    fn pop_enum(&mut self, ty: Enum) -> u32 {
        let persistent = self.pop_persistent();
        with_ctx(|ctx| {
            let name: String = persistent
                .restore(ctx)
                .unwrap()
                .get()
                .expect("expected enum string");
            ty.names()
                .position(|n| name == n)
                .unwrap_or_else(|| panic!("unknown enum case: {name}")) as u32
        })
    }

    fn pop_flags(&mut self, ty: Flags) -> u32 {
        let persistent = self.pop_persistent();
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            let obj = val.as_object().expect("expected flags object");
            let mut bits = 0u32;

            for (i, name) in ty.names().enumerate() {
                let set = obj
                    .get::<_, Coerced<bool>>(fn_lookup(ctx, name))
                    .catch(ctx)
                    .unwrap_or_else(|err| panic!("Failed to read flag '{name}': {err}"))
                    .0;
                if set {
                    bits |= 1 << i;
                }
            }
            bits
        })
    }

    fn pop_borrow(&mut self, ty: Resource) -> u32 {
        let persistent = self.pop_persistent();
        with_ctx(|ctx| {
            let val = persistent.restore(ctx).unwrap();
            if ty.new().is_some() {
                exported_resource_to_handle(ctx, ty, &val)
            } else {
                let (handle, loan) = borrow_imported_resource(ty, &val)
                    .catch(ctx)
                    .expect("Failed to borrow imported resource");
                self.loans.push(loan);
                handle
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
                let (handle, transfer) = transfer_imported_resource(ty, &val)
                    .catch(ctx)
                    .expect("Failed to transfer imported resource");
                self.transfers.push((self.transfer_group, transfer));
                handle
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

    fn pop_future(&mut self, ty: Future) -> u32 {
        self.pop_with(|v| {
            if let Ok(class) = Class::<FutureReadable>::from_value(&v) {
                return class
                    .borrow_mut()
                    .end
                    .begin_transfer(ty.index() as u32)
                    .expect("future cannot be transferred");
            }

            if let Ok(class) = Class::<FutureWritable>::from_value(&v) {
                return class
                    .borrow_mut()
                    .end
                    .begin_transfer(ty.index() as u32)
                    .expect("future cannot be transferred");
            }

            let ctx = v.ctx().clone();

            if is_thenable(&v)
                .catch(&ctx)
                .expect("Failed to inspect future value")
            {
                return crate::futures::lower_thenable(&ctx, ty.index() as u32, v)
                    .catch(&ctx)
                    .expect("Failed to lower WIT future");
            }
            v.get().expect("expected future handle")
        })
    }

    fn pop_stream(&mut self, ty: Stream) -> u32 {
        let persistent = self.pop_persistent();
        with_ctx(|ctx| {
            let v = persistent.restore(ctx).expect("restore stream value");

            if let Ok(class) = Class::<StreamReadable>::from_value(&v) {
                return class
                    .borrow_mut()
                    .end
                    .begin_transfer(ty.index() as u32)
                    .expect("stream cannot be transferred");
            }
            if let Ok(class) = Class::<StreamWritable>::from_value(&v) {
                return class
                    .borrow_mut()
                    .end
                    .begin_transfer(ty.index() as u32)
                    .expect("stream cannot be transferred");
            }

            if is_iterable(ctx, &v)
                .catch(ctx)
                .expect("Failed to inspect stream value")
            {
                return crate::streams::lower_iterable(ctx, ty.index() as u32, v)
                    .catch(ctx)
                    .expect("Failed to lower WIT stream");
            }

            v.get().expect("expected stream handle")
        })
    }

    // Push operations
    fn push_bool(&mut self, val: bool) {
        self.push_with(|ctx| val.into_js(ctx).unwrap());
    }

    fn push_u8(&mut self, val: u8) {
        self.push_with(|ctx| val.into_js(ctx).unwrap());
    }

    fn push_s8(&mut self, val: i8) {
        self.push_with(|ctx| val.into_js(ctx).unwrap());
    }

    fn push_u16(&mut self, val: u16) {
        self.push_with(|ctx| val.into_js(ctx).unwrap());
    }

    fn push_s16(&mut self, val: i16) {
        self.push_with(|ctx| val.into_js(ctx).unwrap());
    }

    fn push_u32(&mut self, val: u32) {
        self.push_with(|ctx| val.into_js(ctx).unwrap());
    }

    fn push_s32(&mut self, val: i32) {
        self.push_with(|ctx| val.into_js(ctx).unwrap());
    }

    fn push_u64(&mut self, val: u64) {
        self.push_with(|ctx| BigInt::from_u64(ctx.clone(), val).unwrap().into_value());
    }

    fn push_s64(&mut self, val: i64) {
        self.push_with(|ctx| BigInt::from_i64(ctx.clone(), val).unwrap().into_value());
    }

    fn push_f32(&mut self, val: f32) {
        self.push_with(|ctx| val.into_js(ctx).unwrap());
    }

    fn push_f64(&mut self, val: f64) {
        self.push_with(|ctx| val.into_js(ctx).unwrap());
    }

    fn push_char(&mut self, val: char) {
        self.push_with(|ctx| val.into_js(ctx).unwrap());
    }

    fn push_string(&mut self, val: String) {
        self.push_with(|ctx| val.into_js(ctx).unwrap());
    }

    unsafe fn push_raw_list(&mut self, ty: List, ptr: *mut u8, len: usize) -> bool {
        match ty.ty() {
            Type::U8 => {
                let vec = unsafe { Vec::from_raw_parts(ptr, len, len) };
                with_ctx(|ctx| {
                    let ta = rquickjs::TypedArray::<u8>::new(ctx.clone(), vec).unwrap();
                    self.stack.push(Persistent::save(ctx, ta.into_value()));
                });
                true
            }
            Type::S8 => {
                let vec = unsafe { Vec::from_raw_parts(ptr.cast::<i8>(), len, len) };
                with_ctx(|ctx| {
                    let ta = rquickjs::TypedArray::<i8>::new(ctx.clone(), vec).unwrap();
                    self.stack.push(Persistent::save(ctx, ta.into_value()));
                });
                true
            }
            _ => false,
        }
    }

    fn push_list(&mut self, _ty: List, _capacity: usize) {
        self.push_with(|ctx| rquickjs::Array::new(ctx.clone()).unwrap().into_value());
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

    fn push_map(&mut self, _ty: Map, _capacity: usize) {
        with_ctx(|ctx| {
            let constructor: Constructor = ctx.globals().get("Map").expect("Map not found");
            let map: Value = constructor.construct(()).expect("failed to create Map");
            self.stack.push(Persistent::save(ctx, map));
        });
    }

    fn map_append(&mut self, _ty: Map) {
        let value = self.stack.pop().expect("stack underflow");
        let key = self.stack.pop().expect("stack underflow");
        let map_persistent = self.stack.last().expect("stack underflow").clone();

        with_ctx(|ctx| {
            let map_value = map_persistent.restore(ctx).unwrap();
            let map = map_value.as_object().expect("expected Map");
            let set: Function = map.get("set").expect("Map.set not found");
            let key = key.restore(ctx).unwrap();
            let value = value.restore(ctx).unwrap();
            let _: Value = set
                .call((This(map.clone()), key, value))
                .expect("failed to append Map entry");
        });
    }

    fn push_option(&mut self, ty: WitOption, is_some: bool) {
        if option_is_nested(ty) {
            // Nested option: wrap so `none` and `some(none)` stay distinct.
            let inner = if is_some { self.stack.pop() } else { None };
            with_ctx(|ctx| {
                let obj = rquickjs::Object::new(ctx.clone()).unwrap();
                if let Some(inner) = inner {
                    obj.set("tag", "some").unwrap();
                    obj.set("val", inner.restore(ctx).unwrap()).unwrap();
                } else {
                    obj.set("tag", "none").unwrap();
                }
                self.stack.push(Persistent::save(ctx, obj.into_value()));
            });
        } else if !is_some {
            // Bare option: `none` is null; `some(v)` leaves `v` on the stack.
            self.push_with(|ctx| Value::new_null(ctx.clone()));
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
        let (name, payload_ty) = ty
            .cases()
            .nth(tag as usize)
            .expect("variant tag out of range");
        let has_payload = payload_ty.is_some();

        let inner = if has_payload { self.stack.pop() } else { None };

        with_ctx(|ctx| {
            let obj = rquickjs::Object::new(ctx.clone()).unwrap();
            obj.set("tag", name).unwrap();
            if let Some(val) = inner {
                obj.set("val", val.restore(ctx).unwrap()).unwrap();
            }
            self.stack.push(Persistent::save(ctx, obj.into_value()));
        });
    }

    fn push_enum(&mut self, ty: Enum, val: u32) {
        let name = ty
            .names()
            .nth(val as usize)
            .expect("enum index out of range");
        self.push_with(move |ctx| name.into_js(ctx).unwrap());
    }

    fn push_flags(&mut self, ty: Flags, val: u32) {
        with_ctx(|ctx| {
            let obj = rquickjs::Object::new(ctx.clone()).unwrap();

            for (i, name) in ty.names().enumerate() {
                obj.set(fn_lookup(ctx, name), (val & (1 << i)) != 0)
                    .unwrap();
            }
            self.stack.push(Persistent::save(ctx, obj.into_value()));
        });
    }

    fn push_borrow(&mut self, ty: Resource, handle: u32) {
        with_ctx(|ctx| {
            let val = if ty.rep().is_some() {
                let rep = handle as usize;
                ctx.resources().get(rep).restore(ctx).unwrap()
            } else {
                let (val, borrow) = make_imported_borrowed(ctx, ty, handle)
                    .catch(ctx)
                    .expect("Failed to wrap borrowed imported resource");
                self.borrows.push(borrow);
                val
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
                    let _ = obj.remove("__cqjs_handle");
                }
                val
            } else {
                make_imported_owned(ctx, ty, handle)
                    .catch(ctx)
                    .expect("Failed to wrap owned imported resource")
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
            let obj = crate::futures::make_future_readable(ctx, ty.index() as u32, handle).unwrap();
            self.stack.push(Persistent::save(ctx, obj.into_value()));
        });
    }

    fn push_stream(&mut self, ty: Stream, handle: u32) {
        with_ctx(|ctx| {
            let obj = crate::streams::make_stream_readable(ctx, ty.index() as u32, handle).unwrap();
            self.stack.push(Persistent::save(ctx, obj.into_value()));
        });
    }
}

fn is_iterable<'js>(ctx: &rquickjs::Ctx<'js>, value: &Value<'js>) -> rquickjs::Result<bool> {
    let Some(object) = value.as_object() else {
        return Ok(false);
    };

    for symbol in [
        Symbol::async_iterator(ctx.clone()),
        Symbol::iterator(ctx.clone()),
    ] {
        if object.get::<_, Value>(symbol.as_atom())?.is_function() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_thenable(value: &Value<'_>) -> rquickjs::Result<bool> {
    let Some(object) = value.as_object() else {
        return Ok(false);
    };
    Ok(object.get::<_, Value>("then")?.is_function())
}
