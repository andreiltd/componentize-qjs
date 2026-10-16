//! Async task state management for inflight export calls.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use wit_dylib_ffi::ExportFunction;

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

/// Settlement recorded by JS jobs and consumed only by the owning export.
#[derive(Default)]
pub(crate) struct ExportCompletion {
    result: Option<(ExportFunction, Persistent<Value<'static>>, bool)>,
    wakeup: Option<u32>,
    settled: bool,
}

impl ExportCompletion {
    pub(crate) fn settle(
        &mut self,
        func: ExportFunction,
        value: Persistent<Value<'static>>,
        rejected: bool,
    ) {
        if self.settled {
            return;
        }
        self.settled = true;
        self.result = Some((func, value, rejected));
        if let Some(writer) = self.wakeup {
            // The owner keeps one unit read pending until completion/cancellation.
            let code = unsafe { wakeup_write(writer, std::ptr::null(), 1) };
            assert_eq!(code, 1 << 4, "export wakeup must complete immediately");
        }
    }
}

/// Inflight operations for one export. Every pending handle is joined to the
/// waitable set until it is removed or temporarily unjoined for cancellation.
#[derive(Default)]
struct TaskInner {
    completion: Rc<RefCell<ExportCompletion>>,
    pending: DetHashMap<u32, PendingEntry>,
    waitable_set: Option<u32>,
    wakeup: Option<u32>,
    export_call: Option<Box<QjsCallContext>>,
    cancel: bool,
    returned: bool,
}

impl TaskInner {
    fn waitable_set(&mut self) -> u32 {
        *self
            .waitable_set
            .get_or_insert_with(|| unsafe { waitable_set_new() })
    }

    /// Sleep on a private stream so a sibling's JS settlement can wake this task.
    fn arm_wakeup(&mut self) {
        if self.wakeup.is_some() {
            return;
        }
        let pair = unsafe { wakeup_new() };
        let writer = (pair >> 32) as u32;
        let reader = pair as u32;
        let code = unsafe { wakeup_read(reader, std::ptr::null_mut(), 1) };
        assert!(is_blocked_raw(code), "export wakeup read must block");
        unsafe { waitable_join(reader, self.waitable_set()) };
        self.wakeup = Some(reader);
        self.completion.borrow_mut().wakeup = Some(writer);
    }

    /// Unjoin before synchronously cancelling, then close both unit endpoints.
    /// `cancel_read` is false when the host has already delivered the read event.
    fn close_wakeup(&mut self, cancel_read: bool) {
        let Some(reader) = self.wakeup.take() else {
            return;
        };
        let writer = self
            .completion
            .borrow_mut()
            .wakeup
            .take()
            .expect("missing wakeup writer");
        unsafe {
            waitable_join(reader, 0);
            if cancel_read {
                wakeup_cancel_read(reader);
            }
            wakeup_drop_readable(reader);
            wakeup_drop_writable(writer);
        }
    }

    /// Store an operation and join its handle to the lazily-created waitable set.
    fn register(&mut self, handle: u32, pending: Pending) {
        assert!(
            !self.pending.contains_key(&handle),
            "operation already pending"
        );

        let set = self.waitable_set();
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

    pub(crate) fn completion(&self) -> Weak<RefCell<ExportCompletion>> {
        // Late JS settlements must not keep a completed or cancelled task alive.
        self.with(|inner| Rc::downgrade(&inner.completion))
    }

    pub(crate) fn finish_export(&self) {
        let call = self.with(|inner| {
            assert!(!inner.cancel && !inner.returned, "task already completed");
            inner.returned = true;
            inner.close_wakeup(true);
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
            inner.close_wakeup(true);
            inner.completion.borrow_mut().settled = true;
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

    fn handle_wakeup(&self, handle: u32) -> bool {
        self.with(|inner| {
            if inner.wakeup != Some(handle) {
                return false;
            }
            inner.close_wakeup(false);
            true
        })
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
            loop {
                while execute_pending_job(ctx).expect("QuickJS job failed") {
                    drain_resource_drops(ctx);
                }
                drain_resource_drops(ctx);
                let completed = self.with(|inner| inner.completion.borrow_mut().result.take());
                if let Some((func, value, rejected)) = completed {
                    if !self.is_cancelling() {
                        let boundary = ResultBoundary::new(func.result());
                        let value = value.restore(ctx).expect("export result restore failed");
                        let value = if rejected {
                            boundary.lower_throw(ctx, value)
                        } else {
                            boundary.lower_value(ctx, value)
                        }
                        .expect("async export result lowering failed");
                        let mut call = QjsCallContext::default();
                        if let Some(value) = value {
                            call.push_value(ctx, value);
                        }
                        self.finish_export();
                        func.call_task_return(&mut call);
                        call.complete_transfers(1);
                    }
                    // Result lowering can start stream/future adapters and queue
                    // more JS jobs. Drain those before deciding to exit or wait.
                    continue;
                }
                break;
            }
        });

        let mut inner = self.0.borrow_mut().take().expect("no active task state");

        if !inner.returned && !inner.cancel {
            inner.arm_wakeup();
        }
        if inner.pending.is_empty() && (inner.returned || inner.cancel) {
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
            let set = inner
                .waitable_set
                .expect("suspended task has no waitable set");
            let ptr = Box::into_raw(Box::new(inner)) as usize;

            unsafe { context_set(u32::try_from(ptr).unwrap()) }
            CallbackCode::Wait.encode(set)
        }
    }
}

/// Dispatch host callbacks, including the private inter-export wakeup stream.
pub(crate) fn handle_event(event: Event) {
    match event {
        Event::None => {}
        Event::Subtask { handle, state } => handle_subtask(handle, state),
        Event::StreamWrite { handle, result } => streams::handle_write_event(handle, result),
        Event::StreamRead { handle, result } => {
            if !with_ctx(|ctx| ctx.task().handle_wakeup(handle)) {
                streams::handle_read_event(handle, result);
            }
        }
        Event::FutureWrite { handle, result } => futures::handle_write_event(handle, result),
        Event::FutureRead { handle, result } => futures::handle_read_event(handle, result),
        Event::TaskCancelled => with_ctx(|ctx| {
            ctx.task()
                .cancel(ctx)
                .catch(ctx)
                .expect("Failed to cancel async task");
        }),
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
