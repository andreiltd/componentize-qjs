//! Async task state management for inflight export calls.
#![allow(unsafe_code)]

use std::cell::RefCell;

use rquickjs::class::Class;
use rquickjs::function::This;
use rquickjs::{CatchResultExt, Ctx, Function, JsLifetime, Persistent, Value};

use crate::CtxExt;
use crate::DetHashMap;
use crate::abi::*;
use crate::buffer::BufferGuard;
use crate::futures::{self, FutureReadable, FutureWritable};
use crate::module::execute_pending_job;
use crate::resources::drain_resource_drops;
use crate::result::ResultBoundary;
use crate::streams::{self, StreamReadable, StreamWritable};
use crate::{QjsCallContext, reject_promise, with_ctx};

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
        resolve: Persistent<Function<'static>>,
        reject: Persistent<Function<'static>>,
    },
    /// A stream write that blocked.
    StreamWrite {
        call: QjsCallContext,
        resolve: Persistent<Function<'static>>,
        wrapper: Persistent<Value<'static>>,
        buffer: BufferGuard,
    },
    /// A stream read that blocked.
    StreamRead {
        call: QjsCallContext,
        buffer: BufferGuard,
        iterator: bool,
        iterator_return: Option<Persistent<Function<'static>>>,
        resolve: Persistent<Function<'static>>,
        wrapper: Persistent<Value<'static>>,
    },
    /// A future write that blocked.
    FutureWrite {
        call: QjsCallContext,
        resolve: Persistent<Function<'static>>,
        wrapper: Persistent<Value<'static>>,
        buffer: BufferGuard,
    },
    /// A future read that blocked.
    FutureRead {
        call: QjsCallContext,
        buffer: BufferGuard,
        resolve: Persistent<Function<'static>>,
        reject: Persistent<Function<'static>>,
        wrapper: Persistent<Value<'static>>,
    },
}

struct PendingEntry {
    op: Pending,
    cancel: bool,
}

enum CancelRequest {
    Import,
    StreamRead(Persistent<Value<'static>>),
    StreamWrite(Persistent<Value<'static>>),
    FutureRead(Persistent<Value<'static>>),
    FutureWrite(Persistent<Value<'static>>),
}

impl Pending {
    fn cancel_request(&self) -> CancelRequest {
        match self {
            Self::ImportCall { .. } => CancelRequest::Import,
            Self::StreamRead { wrapper, .. } => CancelRequest::StreamRead(wrapper.clone()),
            Self::StreamWrite { wrapper, .. } => CancelRequest::StreamWrite(wrapper.clone()),
            Self::FutureRead { wrapper, .. } => CancelRequest::FutureRead(wrapper.clone()),
            Self::FutureWrite { wrapper, .. } => CancelRequest::FutureWrite(wrapper.clone()),
        }
    }
}

impl CancelRequest {
    fn cancel(self, ctx: &Ctx<'_>, handle: u32) -> rquickjs::Result<()> {
        match self {
            Self::Import => {
                ctx.task().unjoin(handle);
                let code = unsafe { subtask_cancel(handle) };
                if is_blocked_raw(code) {
                    ctx.task().rejoin(handle);
                } else {
                    let state = SubtaskState::try_from(code).expect("invalid cancellation state");
                    handle_subtask(handle, state);
                }
            }
            Self::StreamRead(wrapper) => {
                let value = wrapper.restore(ctx)?;
                let class = Class::<StreamReadable>::from_value(&value)?;
                streams::stream_cancel_read(This(class), ctx.clone())?;
            }
            Self::StreamWrite(wrapper) => {
                let value = wrapper.restore(ctx)?;
                let class = Class::<StreamWritable>::from_value(&value)?;
                streams::stream_cancel_write(This(class), ctx.clone())?;
            }
            Self::FutureRead(wrapper) => {
                let value = wrapper.restore(ctx)?;
                let class = Class::<FutureReadable>::from_value(&value)?;
                futures::future_cancel_read(This(class), ctx.clone())?;
            }
            Self::FutureWrite(wrapper) => {
                let value = wrapper.restore(ctx)?;
                let class = Class::<FutureWritable>::from_value(&value)?;
                futures::future_cancel_write(This(class), ctx.clone())?;
            }
        }
        Ok(())
    }
}

/// Inflight async operations for a single export call.
///
/// Every entry is joined to `waitable_set` while the host may wake it.
/// Removing an entry first unjoins its handle so the set and the ownership map
/// remain synchronized.
#[derive(Default)]
struct TaskInner {
    pending: DetHashMap<u32, PendingEntry>,
    waitable_set: Option<u32>,
    export_call: Option<Box<QjsCallContext>>,
    cancel: bool,
    returned: bool,
}

impl TaskInner {
    /// Store an operation and join its handle to the lazily-created waitable set.
    fn register(&mut self, handle: u32, pending: Pending) {
        assert!(
            !self.pending.contains_key(&handle),
            "operation already pending"
        );

        if self.waitable_set.is_none() {
            self.waitable_set = Some(unsafe { waitable_set_new() });
        }

        let set = self.waitable_set.unwrap();
        unsafe { waitable_join(handle, set) };

        self.pending.insert(
            handle,
            PendingEntry {
                op: pending,
                cancel: false,
            },
        );
    }

    /// Unjoin a completed handle and take ownership of its pending state.
    fn take(&mut self, handle: u32) -> Pending {
        unsafe { waitable_join(handle, 0) };
        self.pending
            .remove(&handle)
            .expect("no pending entry for handle")
            .op
    }

    /// Temporarily remove a handle while issuing a cancellation request.
    fn unjoin(&mut self, handle: u32) {
        let pending = self
            .pending
            .get_mut(&handle)
            .expect("no pending entry for handle");
        assert!(!pending.cancel, "operation cancellation already requested");
        pending.cancel = true;
        unsafe { waitable_join(handle, 0) };
    }

    /// Rejoin a handle when its cancellation request also blocks.
    fn rejoin(&mut self, handle: u32) {
        assert!(self.pending.contains_key(&handle));
        unsafe { waitable_join(handle, self.waitable_set.unwrap()) };
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
        self.0.borrow().as_ref().is_some_and(|inner| !inner.cancel)
    }

    pub(crate) fn ensure_active(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        let inner = self.0.borrow();
        match inner.as_ref() {
            Some(inner) if !inner.cancel => Ok(()),
            Some(_) => Err(rquickjs::Exception::throw_message(
                ctx,
                "component task cancelled",
            )),
            None => Err(rquickjs::Exception::throw_type(
                ctx,
                "operation requires an active async call",
            )),
        }
    }

    pub(crate) fn is_cancelling(&self) -> bool {
        self.with(|inner| inner.cancel)
    }

    fn with<R>(&self, f: impl FnOnce(&mut TaskInner) -> R) -> R {
        let mut guard = self.0.borrow_mut();
        f(guard.as_mut().expect("no active task state"))
    }

    /// Initialize a fresh task state for a new async export call.
    pub(crate) fn init(&self, export_call: Box<QjsCallContext>) {
        let mut inner = self.0.borrow_mut();
        assert!(inner.is_none(), "task state already active");

        *inner = Some(TaskInner {
            export_call: Some(export_call),
            ..TaskInner::default()
        });
    }

    pub(crate) fn finish_export(&self) {
        let call = self.with(|inner| {
            assert!(!inner.cancel && !inner.returned, "task already completed");
            inner.returned = true;
            inner.export_call.take()
        });
        drop(call);
    }

    /// Restore task state previously transferred to the host by [`Self::poll`].
    pub(crate) fn restore(&self, ptr: usize) {
        assert_ne!(ptr, 0, "missing suspended task state");
        // `poll` created this allocation with `Box::into_raw`; the host returns
        // the same pointer exactly once on the next callback.
        let inner = unsafe { *Box::from_raw(ptr as *mut TaskInner) };
        let mut state = self.0.borrow_mut();
        assert!(state.is_none(), "task state already active");
        *state = Some(inner);
    }

    /// Request cancellation without releasing buffers still owned by the host.
    pub(crate) fn cancel(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        let requests: Vec<_> = self.with(|inner| {
            inner.cancel = true;
            inner
                .pending
                .iter()
                .filter(|(_, entry)| !entry.cancel)
                .map(|(&handle, entry)| (handle, entry.op.cancel_request()))
                .collect()
        });

        for (handle, request) in requests {
            let pending = self.with(|inner| {
                inner
                    .pending
                    .get(&handle)
                    .is_some_and(|entry| !entry.cancel)
            });
            if pending {
                request.cancel(ctx, handle)?;
            }
        }
        Ok(())
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
        resolve: Persistent<Function<'static>>,
    ) {
        self.with(|inner| {
            let Some(PendingEntry {
                op: Pending::StreamRead {
                    iterator_return, ..
                },
                ..
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
        with_ctx(|ctx| {
            while execute_pending_job(ctx).expect("QuickJS job failed") {
                drain_resource_drops(ctx);
            }
            drain_resource_drops(ctx);
        });

        let mut inner = self.0.borrow_mut().take().expect("no active task state");

        if inner.pending.is_empty() {
            drop(inner.export_call.take());
            with_ctx(drain_resource_drops);

            if inner.cancel && !inner.returned {
                unsafe { task_cancel() };
            }

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
/// cancellation drops the subtask and rejects its promise.
pub(crate) fn handle_subtask(handle: u32, state: SubtaskState) {
    match state {
        SubtaskState::Starting => unreachable!("Starting should not reach callback"),
        SubtaskState::Started => with_ctx(|ctx| {
            ctx.task().with(|inner| {
                let entry = inner.pending.get_mut(&handle).expect("no pending import");
                let Pending::ImportCall { call, .. } = &mut entry.op else {
                    unreachable!("expected ImportCall pending for started subtask");
                };
                call.complete_transfers(1);
            });
        }),
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
            call.complete_transfers(1);

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
            let pending = with_ctx(|ctx| ctx.task().take(handle));
            let Pending::ImportCall {
                reject, mut call, ..
            } = pending
            else {
                unreachable!("expected ImportCall pending for cancelled subtask");
            };

            unsafe { subtask_drop(handle) };

            if state == SubtaskState::CancelledBeforeReturned {
                call.complete_transfers(1);
            }

            drop(call);

            let reason = with_ctx(|ctx| {
                let error =
                    rquickjs::Exception::from_message(ctx.clone(), "async import cancelled")
                        .catch(ctx)
                        .expect("Failed to create cancellation error");
                Persistent::save(ctx, error.into_value())
            });

            reject_promise(reject, reason);
        }
    }
}
