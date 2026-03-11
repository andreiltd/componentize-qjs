//! RAII wrapper for raw buffer allocations used in async ABI operations.
#![allow(unsafe_code)]

use std::alloc::Layout;

/// Owns a `(*mut u8, Layout)` pair and deallocates on `Drop`.
///
/// This prevents memory leaks across async boundaries where a buffer must
/// outlive the allocation site e.g., when a stream/future read/write blocks
/// and the buffer pointer is stored in a `Pending` variant.
pub(crate) struct BufferGuard {
    ptr: *mut u8,
    layout: Layout,
}

impl BufferGuard {
    /// Allocate a new zero-initialized buffer.
    ///
    /// Returns a guard with a dangling pointer for zero-size allocations
    /// https://doc.rust-lang.org/std/alloc/trait.GlobalAlloc.html#tymethod.alloc
    pub(crate) fn new_zeroed(size: usize, align: usize) -> Self {
        let layout = Layout::from_size_align(size, align).expect("invalid layout");
        let ptr = if size == 0 {
            std::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            unsafe { std::alloc::alloc_zeroed(layout) }
        };

        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }

        Self { ptr, layout }
    }

    /// Raw pointer to the buffer.
    pub(crate) fn ptr(&self) -> *mut u8 {
        self.ptr
    }

    /// Consume the guard, returning the raw pointer and layout without
    /// deallocating. The caller assumes ownership of the allocation.
    pub(crate) fn into_raw(self) -> (*mut u8, Layout) {
        let pair = (self.ptr, self.layout);
        std::mem::forget(self);
        pair
    }

    /// Reconstruct a guard from a raw pointer and layout.
    ///
    /// # Safety
    /// The pointer must have been allocated with the given layout or be
    /// dangling if `layout.size() == 0`.
    #[allow(dead_code)]
    pub(crate) unsafe fn from_raw(ptr: *mut u8, layout: Layout) -> Self {
        Self { ptr, layout }
    }

    /// Convert into a `Vec<u8>`, transferring ownership of the allocation.
    ///
    /// # Safety
    /// - Buffer must have been allocated with align=1
    /// - `len` must be ≤ the allocated size
    /// - Buffer contents up to `len` must be initialized
    pub(crate) unsafe fn into_vec(self, len: usize) -> Vec<u8> {
        let (ptr, layout) = self.into_raw();
        if layout.size() == 0 {
            return Vec::new();
        }
        debug_assert_eq!(layout.align(), 1);
        debug_assert!(len <= layout.size());
        unsafe { Vec::from_raw_parts(ptr, len, layout.size()) }
    }
}

impl Drop for BufferGuard {
    fn drop(&mut self) {
        if self.layout.size() > 0 {
            unsafe { std::alloc::dealloc(self.ptr, self.layout) };
        }
    }
}
