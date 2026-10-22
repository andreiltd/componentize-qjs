//! Import argument validation, invocation, and async subtask completion.

use std::rc::Rc;

use rquickjs::object::Property;
use rquickjs::{CatchResultExt, Ctx, Function, Persistent, Value};
use smallvec::SmallVec;
use wit_dylib_ffi::{ImportFunction, Type};

use crate::abi::{SubtaskState, is_blocked_raw, subtask_cancel, subtask_drop};
use crate::resources::validate_imported_resource;
use crate::result::{JsCompletion, ResultBoundary};
use crate::task::Pending;
use crate::{CtxExt, DetHashMap, QjsCallContext, reject_promise, with_ctx};

/// A validated import invocation and its call-lifetime conversion state.
pub(crate) struct ImportCall {
    func: ImportFunction,
    call: QjsCallContext,
}

impl ImportCall {
    /// Validate JS arguments and retain them for lowering.
    pub(crate) fn new<'js>(
        ctx: &Ctx<'js>,
        func: ImportFunction,
        args: SmallVec<[Value<'js>; 8]>,
    ) -> rquickjs::Result<Self> {
        let param_count = func.params().len();
        if args.len() < param_count {
            return Err(rquickjs::Exception::throw_type(
                ctx,
                &format!("{} requires {param_count} arguments", func.name()),
            ));
        }

        let mut resources = DetHashMap::default();
        for (mut ty, arg) in func.params().zip(&args) {
            while let Type::Alias(alias) = ty {
                ty = alias.ty();
            }

            let (resource, owned) = match ty {
                Type::Own(resource) => (resource, true),
                Type::Borrow(resource) => (resource, false),
                _ => continue,
            };

            if resource.new().is_some() {
                continue;
            }

            let handle = validate_imported_resource(resource, arg, owned)?;
            if let Some(was_owned) = resources.insert((resource.index(), handle), owned)
                && (was_owned || owned)
            {
                return Err(rquickjs::Exception::throw_type(
                    ctx,
                    "a transferred resource cannot be used by another argument in the same call",
                ));
            }
        }

        let mut call = QjsCallContext::default();
        for arg in args.into_iter().rev() {
            call.push_value(ctx, arg);
        }

        Ok(Self { func, call })
    }

    /// Invoke the import, returning its result or a promise for it.
    ///
    /// Conversion guards survive result settlement and any async suspension.
    pub(crate) fn invoke<'js>(mut self, ctx: &Ctx<'js>) -> rquickjs::Result<Value<'js>> {
        if self.func.is_async() {
            return self.invoke_async(ctx);
        }

        self.func.call_import_sync(&mut self.call);
        self.mark_started();
        self.lift_result(ctx)?.into_result(ctx)
    }

    /// Start an async import and return a promise for its result.
    ///
    /// 1. Settle the promise immediately if the host returns without blocking.
    /// 2. Otherwise move the call into the task, which owns it while the host
    ///    may still write to the async buffer.
    /// 3. Only then expose `promise.cancel()`, so no failure can release
    ///    buffers the host still owns.
    fn invoke_async<'js>(mut self, ctx: &Ctx<'js>) -> rquickjs::Result<Value<'js>> {
        ctx.task().ensure_active(ctx)?;
        let (promise, resolve, reject) = ctx.promise()?;

        let Some(pending) = (unsafe { self.func.call_import_async(&mut self.call) }) else {
            self.mark_started();
            self.lift_result(ctx)?.settle(&resolve, &reject)?;

            return Ok(promise.into_value());
        };

        let handle = pending.subtask;
        let token = Rc::new(());

        let pending = Pending::ImportCall {
            call: self,
            buffer: pending.buffer,
            resolve: Persistent::save(ctx, resolve),
            reject: Persistent::save(ctx, reject),
            token: token.clone(),
        };

        ctx.task().register(handle, pending);

        // Cancel only this import so its caller can finish asynchronous
        // cleanup. The token prevents cancellation after handle reuse.
        // Define an own data property so no inherited setter runs here.
        let cancel = Function::new(ctx.clone(), move |ctx: Ctx<'_>| {
            ctx.task().cancel_import(&ctx, handle, &token)
        })?;

        promise.prop(
            "cancel",
            Property::from(cancel)
                .writable()
                .configurable()
                .enumerable(),
        )?;

        Ok(promise.into_value())
    }

    /// Commit argument ownership once the host has started consuming the call.
    fn mark_started(&mut self) {
        self.call.complete_transfers(1);
    }

    /// Commit consumed arguments and lift a returned async result.
    ///
    /// Keep the call alive through promise settlement to retain conversion guards.
    fn lift_async_result<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        buffer: *mut u8,
    ) -> rquickjs::Result<JsCompletion<'js>> {
        self.mark_started();
        unsafe { self.func.lift_import_async_result(&mut self.call, buffer) };
        self.lift_result(ctx)
    }

    /// Interpret the converted return value without releasing the call's guards.
    fn lift_result<'js>(&mut self, ctx: &Ctx<'js>) -> rquickjs::Result<JsCompletion<'js>> {
        ResultBoundary::new(self.func.result()).lift(ctx, self.call.maybe_pop_value(ctx)?)
    }
}

/// Request cancellation of a pending import subtask.
///
/// A blocked request rejoins the subtask; its final event settles the promise.
pub(crate) fn cancel_subtask(ctx: &Ctx<'_>, handle: u32) {
    ctx.task().unjoin(handle);
    let code = unsafe { subtask_cancel(handle) };

    if is_blocked_raw(code) {
        ctx.task().rejoin(handle);
    } else {
        let state = SubtaskState::try_from(code).expect("invalid cancellation state");
        handle_subtask(handle, state);
    }
}

/// Reconcile a host subtask event with its pending import promise.
///
/// - `Started`: the host consumed the arguments, so commit their transfers.
/// - `Returned`: lift the result and settle the promise, then release the call.
/// - Cancelled: commit transfers only if the host started, release the call,
///   then reject the promise.
pub(crate) fn handle_subtask(handle: u32, state: SubtaskState) {
    match state {
        SubtaskState::Starting => unreachable!("Starting should not reach callback"),
        SubtaskState::Started => with_ctx(|ctx| {
            ctx.task().with_pending(handle, |pending| {
                let Pending::ImportCall { call, .. } = pending else {
                    unreachable!("expected ImportCall pending for started subtask");
                };

                call.mark_started();
            });
        }),
        SubtaskState::Returned => {
            let pending = with_ctx(|ctx| ctx.task().take(handle));

            let Pending::ImportCall {
                mut call,
                buffer,
                resolve,
                reject,
                ..
            } = pending
            else {
                unreachable!("expected ImportCall pending");
            };

            unsafe { subtask_drop(handle) };

            with_ctx(|ctx| {
                call.lift_async_result(ctx, buffer)
                    .catch(ctx)
                    .expect("Failed to lift async import result")
                    .settle_persistent(ctx, resolve, reject);
            });
        }
        SubtaskState::CancelledBeforeStarted | SubtaskState::CancelledBeforeReturned => {
            let pending = with_ctx(|ctx| ctx.task().take(handle));

            let Pending::ImportCall {
                mut call, reject, ..
            } = pending
            else {
                unreachable!("expected ImportCall pending for cancelled subtask");
            };

            unsafe { subtask_drop(handle) };

            if state == SubtaskState::CancelledBeforeReturned {
                call.mark_started();
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
