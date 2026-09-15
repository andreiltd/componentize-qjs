//! Checked typed-array access and copying into owned ABI buffers.
#![allow(unsafe_code)]

use std::ptr::NonNull;

use rquickjs::{Error, Exception, Result, TypedArray};

use crate::buffer::BufferGuard;

pub(crate) trait TypedArrayExt {
    /// Read the native element count without consulting the JS `length` property.
    fn checked_len(&self) -> Result<usize>;

    fn copy_to_buffer(&self, align: usize) -> Result<(BufferGuard, usize)>;
}

impl<T> TypedArrayExt for TypedArray<'_, T> {
    fn checked_len(&self) -> Result<usize> {
        checked_view(self).map(|(_, count)| count)
    }

    fn copy_to_buffer(&self, align: usize) -> Result<(BufferGuard, usize)> {
        let (bytes, count) = checked_view(self)?;
        let byte_len = bytes.len();
        let buffer = BufferGuard::new_uninit(byte_len, align);

        if byte_len > 0 {
            // SAFETY: the runtime is single-threaded and this Rust allocation
            // cannot execute JS. The source stays live until the disjoint copy
            // finishes; no JS-backed pointer or slice escapes this function.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.cast::<u8>().as_ptr(), buffer.ptr(), byte_len);
            }
        }
        Ok((buffer, count))
    }
}

fn checked_view<T>(array: &TypedArray<'_, T>) -> Result<(NonNull<[u8]>, usize)> {
    let bytes = array.as_raw().ok_or_else(|| {
        if array.ctx().has_exception() {
            Error::Exception
        } else {
            Exception::throw_type(array.ctx(), "typed array backing store is unavailable")
        }
    })?;

    let width = std::mem::size_of::<T>();
    if width == 0 || !bytes.len().is_multiple_of(width) {
        return Err(Exception::throw_type(
            array.ctx(),
            "invalid typed array element layout",
        ));
    }

    Ok((bytes, bytes.len() / width))
}

// Type-specific casts need macros because rquickjs does not export TypedArrayItem.
macro_rules! try_typed_array_copy {
    ($val:expr, $t:ty) => {
        $val.as_object()
            .and_then(|object| object.as_typed_array::<$t>())
            .map(|array| {
                $crate::typed_array::TypedArrayExt::copy_to_buffer(
                    array,
                    std::mem::align_of::<$t>(),
                )
            })
            .transpose()
    };
}

macro_rules! copy_typed_array_as {
    ($obj:expr, $ty:expr, $t:ty) => {
        $obj.as_typed_array::<$t>()
            .map(|array| {
                assert_eq!($ty.abi_payload_size(), std::mem::size_of::<$t>());
                assert!($ty.abi_payload_align() >= std::mem::align_of::<$t>());

                $crate::typed_array::TypedArrayExt::copy_to_buffer(array, $ty.abi_payload_align())
            })
            .transpose()
    };
}

macro_rules! typed_array_len_as {
    ($obj:expr, $t:ty) => {
        $obj.as_typed_array::<$t>()
            .map($crate::typed_array::TypedArrayExt::checked_len)
            .transpose()
    };
}

pub(crate) use {copy_typed_array_as, try_typed_array_copy, typed_array_len_as};
