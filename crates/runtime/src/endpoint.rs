//! Stream/future endpoint ownership and copy-state transitions.

use crate::abi::CopyResult;

/// A rejected endpoint operation, translated to a JS error at the binding boundary.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EndpointError {
    /// Another operation has already used or reserved this endpoint.
    NotIdle(&'static str),
    /// The endpoint belongs to a different WIT stream/future type.
    WrongType(&'static str),
    /// An outstanding operation must settle before disposal.
    InFlight(&'static str),
    /// Only a pending, not-yet-cancelled operation can be cancelled.
    NotCancellable(&'static str),
    /// Ownership has already been transferred or released.
    Dropped,
}

/// The lifecycle of a copy operation, independent of JS and host handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyState {
    Idle,
    Copying,
    Cancelling,
    Done,
}

/// Streams are reusable; futures are consumed by their first successful copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyKind {
    Future,
    Stream,
}

impl CopyKind {
    /// Describe the endpoint in conversion errors.
    fn label(self) -> &'static str {
        match self {
            Self::Future => "future",
            Self::Stream => "stream",
        }
    }
}

/// Owns one canonical endpoint handle and validates its lifecycle transitions.
///
/// State transitions never call the host. Disposal returns the owned handle so
/// callers can release their JS class borrow before invoking its destructor.
pub(crate) struct CopyEnd {
    kind: CopyKind,
    type_index: u32,
    handle: Option<u32>,
    state: CopyState,
}

impl CopyEnd {
    /// Create an idle stream endpoint.
    pub(crate) fn new_stream(type_index: u32, handle: u32) -> Self {
        Self {
            kind: CopyKind::Stream,
            type_index,
            handle: Some(handle),
            state: CopyState::Idle,
        }
    }

    /// Create an idle future endpoint.
    pub(crate) fn new_future(type_index: u32, handle: u32) -> Self {
        Self {
            kind: CopyKind::Future,
            type_index,
            handle: Some(handle),
            state: CopyState::Idle,
        }
    }

    /// Return the index into the endpoint's WIT metadata table.
    pub(crate) fn type_index(&self) -> u32 {
        self.type_index
    }

    /// Whether the endpoint still owns a handle, even if its operation is done.
    pub(crate) fn has_handle(&self) -> bool {
        self.handle.is_some()
    }

    /// Whether no further values can be copied through this endpoint.
    pub(crate) fn is_closed(&self) -> bool {
        self.state == CopyState::Done
    }

    /// Whether an operation or its cancellation still owns the endpoint.
    pub(crate) fn is_pending(&self) -> bool {
        matches!(self.state, CopyState::Copying | CopyState::Cancelling)
    }

    /// Whether an outstanding operation can receive its first cancel request.
    pub(crate) fn can_cancel(&self) -> bool {
        self.state == CopyState::Copying
    }

    /// Validate an idle endpoint and return its handle and WIT type index.
    pub(crate) fn begin_op(&self) -> Result<(u32, u32), EndpointError> {
        if self.state != CopyState::Idle {
            return Err(EndpointError::NotIdle(self.kind.label()));
        }

        let handle = self.handle.ok_or(EndpointError::Dropped)?;
        Ok((handle, self.type_index))
    }

    /// Transfer an idle endpoint to a matching canonical ABI parameter.
    pub(crate) fn begin_transfer(&mut self, type_index: u32) -> Result<u32, EndpointError> {
        if self.type_index != type_index {
            return Err(EndpointError::WrongType(self.kind.label()));
        }

        let (handle, _) = self.begin_op()?;
        self.handle = None;
        self.state = CopyState::Done;
        Ok(handle)
    }

    /// Close a settled endpoint and return its handle exactly once.
    pub(crate) fn begin_drop(&mut self) -> Result<Option<u32>, EndpointError> {
        if self.is_pending() {
            return Err(EndpointError::InFlight(self.kind.label()));
        }

        self.state = CopyState::Done;
        Ok(self.handle.take())
    }

    /// Validate a pending copy before issuing a cancellation request.
    pub(crate) fn begin_cancel(&self) -> Result<(u32, u32), EndpointError> {
        if !self.can_cancel() {
            return Err(EndpointError::NotCancellable(self.kind.label()));
        }

        let handle = self.handle.ok_or(EndpointError::Dropped)?;
        Ok((handle, self.type_index))
    }

    /// Retain ownership while a copy waits for a host callback.
    pub(crate) fn mark_blocked(&mut self) {
        debug_assert_eq!(self.state, CopyState::Idle);
        self.state = CopyState::Copying;
    }

    /// Finish a copy, permitting retries after cancellation but not after closure.
    pub(crate) fn mark_completed(&mut self, result: CopyResult) {
        debug_assert!(!self.is_closed());

        self.state = match (result, self.kind) {
            (CopyResult::Dropped, _) | (CopyResult::Completed, CopyKind::Future) => CopyState::Done,
            (CopyResult::Cancelled, _) | (CopyResult::Completed, CopyKind::Stream) => {
                CopyState::Idle
            }
        };
    }

    /// Retain ownership when the cancellation request itself blocks.
    pub(crate) fn mark_cancel_blocked(&mut self) {
        debug_assert_eq!(self.state, CopyState::Copying);
        self.state = CopyState::Cancelling;
    }
}

#[cfg(test)]
mod tests {
    use quickcheck::{Arbitrary, Gen, TestResult, quickcheck};

    use super::*;

    impl Arbitrary for CopyResult {
        /// Generate one of the three settled copy outcomes.
        fn arbitrary(generator: &mut Gen) -> Self {
            *generator
                .choose(&[Self::Completed, Self::Dropped, Self::Cancelled])
                .unwrap()
        }
    }

    quickcheck! {
        /// Exercise immediate, callback, and cancellation completion for both kinds.
        fn completion_transitions(
            future: bool,
            blocked: bool,
            cancelled: bool,
            result: CopyResult
        ) -> TestResult {
            if cancelled && !blocked {
                return TestResult::discard();
            }

            let mut end = if future {
                CopyEnd::new_future(3, 7)
            } else {
                CopyEnd::new_stream(3, 7)
            };

            assert_eq!(end.begin_op(), Ok((7, 3)));

            if blocked {
                end.mark_blocked();
                assert!(end.begin_op().is_err());
                assert!(end.begin_drop().is_err());
                assert!(end.begin_transfer(3).is_err());
                assert_eq!(end.begin_cancel(), Ok((7, 3)));
            }

            if cancelled {
                end.mark_cancel_blocked();
                assert!(end.begin_cancel().is_err());
                assert!(end.begin_drop().is_err());
            }

            end.mark_completed(result);
            let closed = result == CopyResult::Dropped
                || (future && result == CopyResult::Completed);

            assert_eq!(end.is_closed(), closed);
            assert_eq!(end.begin_op().is_err(), closed);

            assert!(!end.is_pending());
            assert_eq!(end.begin_drop(), Ok(Some(7)));
            assert_eq!(end.begin_drop(), Ok(None));

            TestResult::passed()
        }
    }

    /// Failed transfers preserve ownership; successful transfers consume it.
    #[test]
    fn transfer_ownership() {
        let mut end = CopyEnd::new_stream(3, 7);
        assert_eq!(
            end.begin_transfer(4),
            Err(EndpointError::WrongType("stream"))
        );

        assert_eq!(end.begin_op(), Ok((7, 3)));
        assert_eq!(end.begin_transfer(3), Ok(7));

        assert!(!end.has_handle());
        assert!(end.is_closed());
        assert!(end.begin_op().is_err());
        assert_eq!(end.begin_drop(), Ok(None));
    }
}
