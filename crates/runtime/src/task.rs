//! Async task state management for inflight export calls.
#![allow(unsafe_code)]

use std::cell::RefCell;

use rquickjs::{JsLifetime, Persistent, Value};

use crate::CtxExt;
use crate::DetHashMap;
use crate::abi::*;
use crate::buffer::BufferGuard;
use crate::result::ResultBoundary;
use crate::{QjsCallContext, resolve_promise, with_ctx};

/// A pending async operation awaiting a callback event.
///
/// Each variant owns everything that must survive while control is returned
/// to the host: the conversion stack, any canonical ABI buffer, saved promise
/// callbacks, and the JS endpoint wrapper.
#[allow(dead_code)]
pub(crate) enum Pending {
    /// An async import call that hasn't returned yet.
    ImportCall {
        call: QjsCallContext,
        func_index: usize,
        buffer: *mut u8,
        resolve: Persistent<Value<'static>>,
        reject: Persistent<Value<'static>>,
    },
    /// A stream write that blocked.
    StreamWrite {
        call: QjsCallContext,
        resolve: Persistent<Value<'static>>,
        wrapper: Persistent<Value<'static>>,
        buffer: BufferGuard,
    },
    /// A stream read that blocked.
    StreamRead {
        call: QjsCallContext,
        buffer: BufferGuard,
        iterator: bool,
        iterator_return: Option<Persistent<Value<'static>>>,
        resolve: Persistent<Value<'static>>,
        wrapper: Persistent<Value<'static>>,
    },
    /// A future write that blocked.
    FutureWrite {
        call: QjsCallContext,
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
///
/// Every entry is joined to `waitable_set` while the host may wake it.
/// Removing an entry first unjoins its handle so the set and the ownership map
/// remain synchronized.
#[derive(Default)]
struct TaskInner {
    pending: DetHashMap<u32, Pending>,
    waitable_set: Option<u32>,
}

impl TaskInner {
    /// Store an operation and join its handle to the lazily-created waitable set.
    fn register(&mut self, handle: u32, pending: Pending) {
        if self.waitable_set.is_none() {
            self.waitable_set = Some(unsafe { waitable_set_new() });
        }
        let set = self.waitable_set.unwrap();
        unsafe { waitable_join(handle, set) };
        self.pending.insert(handle, pending);
    }

    /// Unjoin a completed handle and take ownership of its pending state.
    fn take(&mut self, handle: u32) -> Pending {
        unsafe { waitable_join(handle, 0) };
        self.pending
            .remove(&handle)
            .expect("no pending entry for handle")
    }

    /// Temporarily remove a handle while issuing a cancellation request.
    fn unjoin(&mut self, handle: u32) {
        assert!(self.pending.contains_key(&handle));
        unsafe { waitable_join(handle, 0) };
    }

    /// Rejoin a handle when its cancellation request also blocks.
    fn rejoin(&mut self, handle: u32) {
        assert!(self.pending.contains_key(&handle));
        unsafe { waitable_join(handle, self.waitable_set.unwrap()) };
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

/// Task state for the active async export.
///
/// The state lives here while QuickJS is running. [`TaskState::poll`] moves it
/// into a host context pointer while waiting, and [`TaskState::restore`] moves
/// it back when the callback resumes.
#[derive(JsLifetime)]
pub(crate) struct TaskState(RefCell<Option<TaskInner>>);

impl TaskState {
    pub(crate) const fn new() -> Self {
        Self(RefCell::new(None))
    }

    pub(crate) fn is_active(&self) -> bool {
        self.0.borrow().is_some()
    }

    fn with<R>(&self, f: impl FnOnce(&mut TaskInner) -> R) -> R {
        let mut guard = self.0.borrow_mut();
        f(guard.as_mut().expect("no active task state"))
    }

    /// Initialize a fresh task state for a new async export call.
    pub(crate) fn init(&self) {
        *self.0.borrow_mut() = Some(TaskInner::default());
    }

    /// Restore task state previously transferred to the host by [`Self::poll`].
    pub(crate) fn restore(&self, ptr: usize) {
        // `poll` created this allocation with `Box::into_raw`; the host returns
        // the same pointer exactly once on the next callback.
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

    pub(crate) fn unjoin(&self, handle: u32) {
        self.with(|inner| inner.unjoin(handle));
    }

    pub(crate) fn rejoin(&self, handle: u32) {
        self.with(|inner| inner.rejoin(handle));
    }

    /// Attach an async-iterator `return()` resolver to its pending stream read.
    ///
    /// The read callback completes both the original `next()` and the deferred
    /// `return()` after cancellation has settled.
    pub(crate) fn set_stream_iterator_return(
        &self,
        handle: u32,
        resolve: Persistent<Value<'static>>,
    ) {
        self.with(|inner| {
            let Some(Pending::StreamRead {
                iterator_return, ..
            }) = inner.pending.get_mut(&handle)
            else {
                panic!("no pending stream read for handle");
            };
            assert!(
                iterator_return.replace(resolve).is_none(),
                "stream iterator return already pending"
            );
        });
    }

    /// Drain the QuickJS job queue and either finish or suspend the export.
    ///
    /// Suspending transfers `TaskInner` into a raw host-context pointer. No Rust
    /// owner remains until the host supplies that pointer to [`Self::restore`].
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

/// Reconcile a host subtask event with its pending JavaScript import promise.
///
/// Returned subtasks lift their canonical result before settling the promise;
/// cancellation drops the subtask and resolves with `undefined`.
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
                reject,
                mut call,
            } = pending
            else {
                unreachable!("expected ImportCall pending");
            };

            let func = with_ctx(|ctx| ctx.wit()).import_func(func_index);
            unsafe { func.lift_import_async_result(&mut call, buffer) };

            with_ctx(|ctx| {
                ResultBoundary::new(func.result())
                    .lift(ctx, call.maybe_pop_value(ctx).unwrap())
                    .unwrap()
                    .settle_persistent(ctx, resolve, reject);
            });
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
