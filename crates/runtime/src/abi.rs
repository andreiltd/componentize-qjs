//! WASI Component Model async ABI: FFI imports, protocol enums, and event decoding.
//!
//! Canonical ABI specification:
//! <https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md>
#![allow(unsafe_code)]

use num_enum::TryFromPrimitive;

/// Returned by stream/future read/write when the operation blocks.
pub(crate) const BLOCKED: u32 = 0xFFFF_FFFF;

#[inline(always)]
pub(crate) fn is_blocked_raw(code: u32) -> bool {
    code == BLOCKED
}

/// Event codes delivered as `event0` in the async callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
#[repr(u32)]
enum EventCode {
    None = 0,
    Subtask = 1,
    StreamRead = 2,
    StreamWrite = 3,
    FutureRead = 4,
    FutureWrite = 5,
    TaskCancelled = 6,
}

/// Subtask state transitions, this is delivered in `event2` when event type is `Subtask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
#[repr(u32)]
pub(crate) enum SubtaskState {
    Starting = 0,
    Started = 1,
    Returned = 2,
    CancelledBeforeStarted = 3,
    CancelledBeforeReturned = 4,
}

/// Callback return codes, returned from the `canon lift` callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
#[allow(dead_code)]
pub(crate) enum CallbackCode {
    Exit = 0,
    Yield = 1,
    Wait = 2,
}

impl CallbackCode {
    /// Encode as the raw `u32` returned from the callback.
    /// `Wait` encodes the waitable set index in the upper bits.
    pub(crate) fn encode(self, waitable_set: u32) -> u32 {
        match self {
            Self::Exit => 0,
            Self::Yield => 1,
            Self::Wait => 2 | (waitable_set << 4),
        }
    }
}

/// Stream/future I/O result codes, this is the lower 4 bits of packed return value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
#[repr(u32)]
pub(crate) enum CopyResult {
    Completed = 0,
    Dropped = 1,
    Cancelled = 2,
}

/// Unpack a stream/future return value into (progress, result) tuple.
///
/// Returns `None` if the operation blocked.
pub(crate) fn unpack_copy_result(packed: u32) -> Option<(u32, CopyResult)> {
    if is_blocked_raw(packed) {
        return None;
    }

    let progress = packed >> 4;
    let result = CopyResult::try_from(packed & 0xF).expect("unknown copy result");

    Some((progress, result))
}

/// Tracks the lifecycle of a copy operation on a stream/future end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
#[repr(u32)]
#[allow(dead_code)]
pub(crate) enum CopyState {
    Idle = 1,
    SyncCopying = 2,
    AsyncCopying = 3,
    CancellingCopy = 4,
    Done = 5,
}

impl CopyState {
    /// Whether a copy operation is currently in progress.
    pub(crate) fn copying(self) -> bool {
        !matches!(self, Self::Idle | Self::Done)
    }

    /// Whether an active copy operation can be cancelled.
    pub(crate) fn cancellable(self) -> bool {
        matches!(self, CopyState::AsyncCopying)
    }
}

/// Whether a copy end allows re-use after a completed operation.
///
/// Streams are reusable (can do more reads/writes after completion),
/// futures are one-shot (always transition to Done).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyKind {
    Future,
    Stream,
}

impl CopyKind {
    fn completion(self) -> CopyState {
        match self {
            // Futures are one-shot, they can't be used again after completion.
            CopyKind::Future => CopyState::Done,
            // Streams are reusable, they can be used again after completion.
            CopyKind::Stream => CopyState::Idle,
        }
    }

    fn label(self) -> &'static str {
        match self {
            CopyKind::Future => "future",
            CopyKind::Stream => "stream",
        }
    }
}

/// Common state for a readable or writable stream/future end.
pub(crate) struct CopyEnd {
    kind: CopyKind,
    pub(crate) type_index: u32,
    pub(crate) handle: Option<u32>,
    pub(crate) state: CopyState,
}

impl CopyEnd {
    /// Create a new stream end in the Idle state.
    pub(crate) fn new_stream(type_index: u32, handle: u32) -> Self {
        Self {
            kind: CopyKind::Stream,
            type_index,
            handle: Some(handle),
            state: CopyState::Idle,
        }
    }

    /// Create a new future end in the Idle state.
    pub(crate) fn new_future(type_index: u32, handle: u32) -> Self {
        Self {
            kind: CopyKind::Future,
            type_index,
            handle: Some(handle),
            state: CopyState::Idle,
        }
    }

    /// Validate that the end is idle and ready for a new read/write.
    /// Returns `(handle, type_index)` on success.
    pub(crate) fn begin_op(&self) -> rquickjs::Result<(u32, u32)> {
        if self.state.copying() {
            return Err(rquickjs::Error::new_from_js(
                self.kind.label(),
                "operation while copy in progress",
            ));
        }

        let h = self
            .handle
            .ok_or_else(|| rquickjs::Error::new_from_js("object", "already dropped"))?;

        Ok((h, self.type_index))
    }

    /// Validate that the end has an active async copy that can be cancelled.
    /// Returns `(handle, type_index)` on success.
    pub(crate) fn begin_cancel(&self) -> rquickjs::Result<(u32, u32)> {
        if !self.state.cancellable() {
            return Err(rquickjs::Error::new_from_js(
                self.kind.label(),
                "cancel without active operation",
            ));
        }

        let h = self
            .handle
            .ok_or_else(|| rquickjs::Error::new_from_js("object", "already dropped"))?;

        Ok((h, self.type_index))
    }

    /// Transition to AsyncCopying when the operation blocks.
    pub(crate) fn mark_blocked(&mut self) {
        debug_assert_eq!(self.state, CopyState::Idle);
        self.state = CopyState::AsyncCopying;
    }

    /// Transition after a sync or async completion.
    pub(crate) fn mark_completed(&mut self, result: CopyResult) {
        self.state = match result {
            CopyResult::Dropped => CopyState::Done,
            CopyResult::Cancelled => CopyState::Idle,
            CopyResult::Completed => self.kind.completion(),
        };
    }

    /// Transition to CancellingCopy when a cancel itself blocks.
    pub(crate) fn mark_cancel_blocked(&mut self) {
        debug_assert_eq!(self.state, CopyState::AsyncCopying);
        self.state = CopyState::CancellingCopy;
    }
}

/// A decoded async callback event
pub(crate) enum Event {
    None,
    Subtask { handle: u32, state: SubtaskState },
    StreamRead { handle: u32, result: u32 },
    StreamWrite { handle: u32, result: u32 },
    FutureRead { handle: u32, result: u32 },
    FutureWrite { handle: u32, result: u32 },
    TaskCancelled,
}

impl Event {
    /// Decode a raw `(event0, event1, event2)` callback triple into a typed event.
    pub(crate) fn decode(event0: u32, event1: u32, event2: u32) -> Self {
        match EventCode::try_from(event0).expect("unknown event code") {
            EventCode::None => Self::None,
            EventCode::Subtask => Self::Subtask {
                handle: event1,
                state: SubtaskState::try_from(event2).expect("unknown subtask state"),
            },
            EventCode::StreamRead => Self::StreamRead {
                handle: event1,
                result: event2,
            },
            EventCode::StreamWrite => Self::StreamWrite {
                handle: event1,
                result: event2,
            },
            EventCode::FutureRead => Self::FutureRead {
                handle: event1,
                result: event2,
            },
            EventCode::FutureWrite => Self::FutureWrite {
                handle: event1,
                result: event2,
            },
            EventCode::TaskCancelled => Self::TaskCancelled,
        }
    }
}

// Canonical built-in imports
#[link(wasm_import_module = "$root")]
unsafe extern "C" {
    #[link_name = "[waitable-set-new]"]
    pub(crate) fn waitable_set_new() -> u32;
}

#[link(wasm_import_module = "$root")]
unsafe extern "C" {
    #[link_name = "[waitable-join]"]
    pub(crate) fn waitable_join(waitable: u32, set: u32);
}

#[link(wasm_import_module = "$root")]
unsafe extern "C" {
    #[link_name = "[waitable-set-drop]"]
    pub(crate) fn waitable_set_drop(set: u32);
}

#[link(wasm_import_module = "$root")]
unsafe extern "C" {
    #[link_name = "[waitable-set-poll]"]
    #[allow(dead_code)]
    pub(crate) fn waitable_set_poll(set: u32, payload_addr: *mut u32) -> u32;
}

#[link(wasm_import_module = "$root")]
unsafe extern "C" {
    #[link_name = "[waitable-set-wait]"]
    #[allow(dead_code)]
    pub(crate) fn waitable_set_wait(set: u32, payload_addr: *mut u32) -> u32;
}

#[link(wasm_import_module = "$root")]
unsafe extern "C" {
    #[link_name = "[subtask-drop]"]
    pub(crate) fn subtask_drop(task: u32);
}

#[link(wasm_import_module = "$root")]
unsafe extern "C" {
    #[link_name = "[subtask-cancel]"]
    #[allow(dead_code)]
    pub(crate) fn subtask_cancel(task: u32) -> u32;
}

#[link(wasm_import_module = "$root")]
unsafe extern "C" {
    #[link_name = "[context-get-0]"]
    pub(crate) fn context_get() -> u32;
}

#[link(wasm_import_module = "$root")]
unsafe extern "C" {
    #[link_name = "[context-set-0]"]
    pub(crate) fn context_set(value: u32);
}

#[link(wasm_import_module = "[export]$root")]
unsafe extern "C" {
    #[link_name = "[task-cancel]"]
    #[allow(dead_code)]
    pub(crate) fn task_cancel();
}

#[link(wasm_import_module = "$root")]
unsafe extern "C" {
    #[link_name = "[thread-yield]"]
    #[allow(dead_code)]
    pub(crate) fn thread_yield() -> u32;
}

#[link(wasm_import_module = "[export]$root")]
unsafe extern "C" {
    #[link_name = "[backpressure-set]"]
    #[allow(dead_code)]
    pub(crate) fn backpressure_set(enabled: u32);
}

// WASI adapter state reset that is used during Wizer pre-initialization
#[link(wasm_import_module = "wasi_snapshot_preview1")]
unsafe extern "C" {
    #[link_name = "reset_adapter_state"]
    pub(crate) fn reset_adapter_state();
}

unsafe extern "C" {
    pub(crate) fn __wasilibc_reset_preopens();
}
