//! Async task state management for inflight export calls.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;

use rquickjs::{JsLifetime, Persistent, Value};

use crate::CtxExt;
use crate::abi::*;
use crate::buffer::BufferGuard;
use crate::{QjsCallContext, resolve_promise, with_ctx};

/// A pending async operation awaiting a callback event.
#[allow(dead_code)]
pub(crate) enum Pending {
    /// An async import call that hasn't returned yet.
    ImportCall {
        call: QjsCallContext,
        func_index: usize,
        buffer: *mut u8,
        resolve: Persistent<Value<'static>>,
    },
    /// A stream write that blocked.
    StreamWrite {
        resolve: Persistent<Value<'static>>,
        wrapper: Persistent<Value<'static>>,
        buffer: BufferGuard,
    },
    /// A stream read that blocked.
    StreamRead {
        call: QjsCallContext,
        buffer: BufferGuard,
        resolve: Persistent<Value<'static>>,
        wrapper: Persistent<Value<'static>>,
    },
    /// A future write that blocked.
    FutureWrite {
        resolve: Persistent<Value<'static>>,
        wrapper: Persistent<Value<'static>>,
        buffer: BufferGuard,
    },
    /// A future read that blocked.
    FutureRead {
        call: QjsCallContext,
        buffer: BufferGuard,
        resolve: Persistent<Value<'static>>,
        reject: Persistent<Value<'static>>,
        wrapper: Persistent<Value<'static>>,
    },
}

/// Inflight async operations for a single export call.
#[derive(Default)]
struct TaskInner {
    pending: HashMap<u32, Pending>,
    waitable_set: Option<u32>,
}

impl TaskInner {
    fn register(&mut self, handle: u32, pending: Pending) {
        if self.waitable_set.is_none() {
            self.waitable_set = Some(unsafe { waitable_set_new() });
        }
        let set = self.waitable_set.unwrap();
        unsafe { waitable_join(handle, set) };
        self.pending.insert(handle, pending);
    }

    fn take(&mut self, handle: u32) -> Pending {
        unsafe { waitable_join(handle, 0) };
        self.pending
            .remove(&handle)
            .expect("no pending entry for handle")
    }

    fn cancel(&mut self) {
        for &handle in self.pending.keys() {
            unsafe { waitable_join(handle, 0) };
        }
        self.pending.clear();
        if let Some(set) = self.waitable_set.take() {
            unsafe { waitable_set_drop(set) }
        }
    }
}

/// Global task state
#[derive(JsLifetime)]
pub(crate) struct TaskState(RefCell<Option<TaskInner>>);

impl TaskState {
    pub(crate) const fn new() -> Self {
        Self(RefCell::new(None))
    }

    fn with<R>(&self, f: impl FnOnce(&mut TaskInner) -> R) -> R {
        let mut guard = self.0.borrow_mut();
        f(guard.as_mut().expect("no active task state"))
    }

    /// Initialize a fresh task state for a new async export call.
    pub(crate) fn init(&self) {
        *self.0.borrow_mut() = Some(TaskInner::default());
    }

    /// Restore a previously saved task state from host context pointer.
    pub(crate) fn restore(&self, ptr: usize) {
        let inner = unsafe { *Box::from_raw(ptr as *mut TaskInner) };
        *self.0.borrow_mut() = Some(inner);
    }

    /// Cancel and clean up the current task state.
    pub(crate) fn cancel(&self) {
        self.with(|inner| inner.cancel());
    }

    /// Register a pending operation, joining it to the waitable set.
    pub(crate) fn register(&self, handle: u32, pending: Pending) {
        self.with(|inner| inner.register(handle, pending));
    }

    /// Unjoin a handle and remove its pending operation.
    pub(crate) fn take(&self, handle: u32) -> Pending {
        self.with(|inner| inner.take(handle))
    }

    /// Drive the quickjs job queue until drained, then decide whether to
    /// exit or wait for more events. Returns the encoded callback code.
    pub(crate) fn poll(&self) -> u32 {
        with_ctx(|ctx| while ctx.execute_pending_job() {});

        let mut inner = self.0.borrow_mut().take().expect("no active task state");

        if inner.pending.is_empty() {
            if let Some(set) = inner.waitable_set.take() {
                unsafe { waitable_set_drop(set) }
            }
            CallbackCode::Exit.encode(0)
        } else {
            let set = inner.waitable_set.expect("pending ops but no waitable set");
            let ptr = Box::into_raw(Box::new(inner)) as usize;

            unsafe { context_set(u32::try_from(ptr).unwrap()) }
            CallbackCode::Wait.encode(set)
        }
    }
}

/// Handle a subtask (async import call) event.
pub(crate) fn handle_subtask(handle: u32, state: SubtaskState) {
    match state {
        SubtaskState::Starting => unreachable!("Starting should not reach callback"),
        SubtaskState::Started => { /* subtask started, nothing to do yet */ }
        SubtaskState::Returned => {
            let pending = with_ctx(|ctx| ctx.task().take(handle));
            unsafe { subtask_drop(handle) };

            let Pending::ImportCall {
                func_index,
                buffer,
                resolve,
                mut call,
            } = pending
            else {
                unreachable!("expected ImportCall pending");
            };

            let func = with_ctx(|ctx| ctx.wit()).import_func(func_index);
            unsafe { func.lift_import_async_result(&mut call, buffer) };

            let result = func.result().map(|_| call.pop_persistent());
            resolve_promise(resolve, result);
        }
        SubtaskState::CancelledBeforeStarted | SubtaskState::CancelledBeforeReturned => {
            let Pending::ImportCall { resolve, .. } = with_ctx(|ctx| ctx.task().take(handle))
            else {
                unreachable!("expected ImportCall pending for cancelled subtask");
            };

            unsafe { subtask_drop(handle) };
            resolve_promise(resolve, None);
        }
    }
}
