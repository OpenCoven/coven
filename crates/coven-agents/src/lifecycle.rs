//! Bounded lifecycle for one invocation: an injectable clock, cooperative
//! cancellation, and wall-clock deadlines.
//!
//! The runner observes a stop only at checkpoints: before each model turn,
//! after each model response, and before each tool dispatch. Tools and models
//! can observe the same [`RunSignal`] while they run and return early. The
//! runner never preempts a future and never retries.
//!
//! When a stop is observed with no tool in flight, the invocation ends as
//! [`crate::RunFailureKind::Cancelled`]. Tool effects that completed before
//! that point stay in the partial transcript; cancellation does not mean
//! nothing happened. When a tool fails after a stop was requested while it
//! ran, the runner cannot know whether its effect happened, so the invocation
//! ends as [`crate::RunFailureKind::TerminationUnknown`] unless the tool
//! returns [`NotStarted`] to assert it produced no effect.

use std::{
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Monotonic time source. `now` is an offset from an arbitrary origin that
/// never moves backwards; only differences between readings are meaningful.
pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

/// The process's monotonic clock.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    origin: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// A clock that moves only when told to, for deterministic deadline tests.
#[derive(Debug, Default)]
pub struct ManualClock {
    now: Mutex<Duration>,
}

impl ManualClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves the clock forward by `by`.
    pub fn advance(&self, by: Duration) {
        let mut now = self
            .now
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *now = now.saturating_add(by);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        *self
            .now
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug, Default)]
struct TokenNode {
    cancelled: AtomicBool,
    parent: Option<Arc<TokenNode>>,
}

impl TokenNode {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.is_cancelled())
    }
}

/// A cooperative cancellation request shared between a caller and a run.
///
/// Cancelling a token cancels every child token derived from it; cancelling
/// a child never cancels its parent. Cancellation is a request: the runner
/// acknowledges it at its next checkpoint.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    node: Arc<TokenNode>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// A token that is cancelled whenever `self` is, and can also be
    /// cancelled on its own.
    pub fn child(&self) -> Self {
        Self {
            node: Arc::new(TokenNode {
                cancelled: AtomicBool::new(false),
                parent: Some(Arc::clone(&self.node)),
            }),
        }
    }

    pub fn cancel(&self) {
        self.node.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.node.is_cancelled()
    }
}

/// Why an invocation was asked to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// A caller cancelled the invocation's token or an ancestor of it.
    Cancelled,
    /// The invocation's deadline passed.
    DeadlineExceeded,
}

impl fmt::Display for StopReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "cancelled",
            Self::DeadlineExceeded => "deadline exceeded",
        })
    }
}

/// Per-run cancellation and deadline. The default never stops a run.
#[derive(Debug, Clone, Default)]
pub struct RunControls {
    cancellation: Option<CancellationToken>,
    timeout: Option<Duration>,
}

impl RunControls {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    /// Bounds the run to `timeout` from when it starts, measured on the
    /// runner's clock.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn cancellation(&self) -> Option<&CancellationToken> {
        self.cancellation.as_ref()
    }

    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }
}

/// What a running invocation, and the tools and models inside it, can
/// observe about whether it should stop.
#[derive(Clone)]
pub struct RunSignal {
    cancellation: Option<CancellationToken>,
    clock: Arc<dyn Clock>,
    deadline: Option<Duration>,
}

impl fmt::Debug for RunSignal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunSignal")
            .field("cancellation", &self.cancellation)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl RunSignal {
    /// Starts a signal whose deadline, if any, begins on `clock` now. The
    /// runner starts one per run; a host that orchestrates delegation outside
    /// any run can start one to derive child controls from.
    pub fn new(controls: RunControls, clock: Arc<dyn Clock>) -> Self {
        let deadline = controls
            .timeout
            .map(|timeout| clock.now().saturating_add(timeout));
        Self {
            cancellation: controls.cancellation,
            clock,
            deadline,
        }
    }

    /// The reason to stop, if any. Cancellation is reported ahead of an
    /// expired deadline when both hold.
    pub fn stop_reason(&self) -> Option<StopReason> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Some(StopReason::Cancelled);
        }
        match self.deadline {
            Some(deadline) if self.clock.now() >= deadline => Some(StopReason::DeadlineExceeded),
            _ => None,
        }
    }

    pub fn is_stop_requested(&self) -> bool {
        self.stop_reason().is_some()
    }

    /// Time left before the deadline, or `None` when the run has none.
    pub fn remaining(&self) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_sub(self.clock.now()))
    }

    /// Controls for a child invocation started from this one. Cancelling
    /// this run's token cancels the child; cancelling the child does not
    /// cancel this run. The child's timeout is whatever remains of this
    /// run's deadline, so a child never outlives its parent's bound.
    pub fn child_controls(&self) -> RunControls {
        RunControls {
            cancellation: self.cancellation.as_ref().map(CancellationToken::child),
            timeout: self.remaining(),
        }
    }
}

/// A tool's explicit assertion that it stopped before producing any effect.
///
/// Return it, boxed, from `Tool::execute_with_signal` after observing a stop.
/// The runner then acknowledges a clean cancellation. Any other error after a
/// stop leaves the effect unknown: errors, timeouts, and disconnection never
/// prove that nothing happened.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("tool stopped before producing any effect: {reason}")]
pub struct NotStarted {
    reason: String,
}

impl NotStarted {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_tokens_follow_their_ancestors_but_not_the_reverse() {
        let root = CancellationToken::new();
        let child = root.child();
        let grandchild = child.child();
        let sibling = root.child();

        grandchild.cancel();
        assert!(grandchild.is_cancelled());
        assert!(!child.is_cancelled());
        assert!(!root.is_cancelled());

        root.cancel();
        assert!(child.is_cancelled());
        assert!(sibling.is_cancelled());
    }

    #[test]
    fn signal_reports_deadline_on_the_injected_clock_and_prefers_cancellation() {
        let clock = Arc::new(ManualClock::new());
        clock.advance(Duration::from_secs(100));
        let cancellation = CancellationToken::new();
        let signal = RunSignal::new(
            RunControls::new()
                .with_cancellation(cancellation.clone())
                .with_timeout(Duration::from_secs(10)),
            clock.clone(),
        );

        assert_eq!(signal.stop_reason(), None);
        assert_eq!(signal.remaining(), Some(Duration::from_secs(10)));
        clock.advance(Duration::from_secs(9));
        assert_eq!(signal.stop_reason(), None);
        assert_eq!(signal.remaining(), Some(Duration::from_secs(1)));
        clock.advance(Duration::from_secs(1));
        assert_eq!(signal.stop_reason(), Some(StopReason::DeadlineExceeded));
        assert_eq!(signal.remaining(), Some(Duration::ZERO));
        cancellation.cancel();
        assert_eq!(signal.stop_reason(), Some(StopReason::Cancelled));
    }

    #[test]
    fn default_controls_never_stop() {
        let signal = RunSignal::new(RunControls::default(), Arc::new(ManualClock::new()));
        assert_eq!(signal.stop_reason(), None);
        assert_eq!(signal.remaining(), None);
    }
}
