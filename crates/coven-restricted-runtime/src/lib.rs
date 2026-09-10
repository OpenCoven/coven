//! Single-use startup/control, not OS enforcement or session-policy support.
//!
//! See the crate README for the trusted backend, sealed offline runtime,
//! independent guardian, explicit cleanup, and drop obligations.
#![forbid(unsafe_code)]

use std::fmt;
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};

/// Reviewed backend metadata, never a permission grant or physical proof.
///
/// The backend must allocate a fresh, never-reused attempt/worker identity and
/// retain the physical resources it names. IDs must not contain private data.
/// The controller stores an immutable copy, separately from driver reports.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub attempt: u128,
    pub backend: u128,
    pub worker: u128,
    pub workspace: u128,
    pub executable: u128,
    pub runtime_closure: u128,
    pub stdio_pipes: u128,
}

impl fmt::Debug for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Binding { .. }")
    }
}

/// Milliseconds in one monotonic clock domain, not Unix/wall-clock time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    pub era: u64,
    pub millis: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockError {
    Unavailable,
    Overflow,
}

/// A trusted clock must report elapsed monotonic time even between polls.
/// A frozen/fake clock cannot enforce expiry. Never substitute a wall clock.
pub trait Clock {
    fn observe(&mut self) -> Result<Reading, ClockError>;
}

/// Fixed at start, including preparation time; never renewable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub origin: Reading,
    pub deadline_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Prepare,
    Install,
    RetainLifetime,
    Revalidate,
    Execute,
    Cleanup,
    ObserveTermination,
}

/// Correlation only. Echoing this value does not prove enforcement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub binding: Binding,
    pub operation: Operation,
    pub sequence: u64,
}

/// Fixed offline launch contract; no shell, URL, argv or environment options.
///
/// Startup always has a lease. Cleanup following an invalid start may not.
/// Only the bound controller stdio pipes may be inherited; every other
/// descriptor must be closed, not inherited merely because it already exists.
#[derive(Clone)]
pub struct Request {
    pub receipt: Receipt,
    pub lease: Option<Lease>,
    signal: StopSignal,
}

impl Request {
    pub fn initial_environment(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    pub fn stdio_pipe_identity(&self) -> u128 {
        self.receipt.binding.stdio_pipes
    }

    /// Clone for the independent guardian; notification is not termination.
    pub fn stop_signal(&self) -> &StopSignal {
        &self.signal
    }
}

/// Backend errors must be translated here, never returned as diagnostic text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendError {
    Unavailable,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation {
    Binding,
    Clock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidLease,
    DeadlineOverflow,
    Expired,
    ClockRegressed,
    ClockChanged,
    Clock(ClockError),
    BindingChanged,
    Cancelled,
    OwnerLost,
    AlreadyStarted,
    InvalidState,
    CleanupAlreadyRequested,
    StopAlreadySignalled,
    SequenceExhausted,
    Backend(Operation, BackendError),
    InvalidReceipt(Operation),
    CallbackInterrupted(Operation),
    ObservationInterrupted(Observation),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ready,
    Preparing,
    Installing,
    RetainingLifetime,
    Revalidating,
    Running,
    Refused,
    Stopping,
    Terminated,
}

/// Sticky historical fact: cleanup or termination never erases the handoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Execution {
    NotStarted,
    PossiblyStarted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupState {
    NotRequested,
    Pending,
    Released,
    Unknown,
}

/// Bounded snapshot, not a live clock/signal query. `reason` retains the first
/// fence cause; later operation errors are returned to the caller separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    pub execution: Execution,
    pub cleanup: CleanupState,
    pub reason: Option<Error>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupOutcome {
    Pending,
    /// Resources released; not an observation of whole-worker termination.
    Released,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Termination {
    Pending,
    /// Trusted observation that the whole bound worker lifetime is terminated.
    /// A signal sent, stopped parent, or cleanup acknowledgment is insufficient.
    Confirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CleanupReport {
    pub receipt: Receipt,
    pub outcome: CleanupOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminationReport {
    pub receipt: Receipt,
    pub outcome: Termination,
}

/// The only resource/OS seam. Implementations are trusted, not plugins.
///
/// Keep custody of all resources internally, including on errors/unwind.
/// Never execute before `execute`, including during preparation, installation,
/// lifetime retention, or cleanup. All callbacks must be bounded.
///
/// `prepare` seals the complete executable/runtime/config resource closure under
/// the workspace, without ambient reads or cloud inference. `install_restrictions`
/// installs whole-worker offline restrictions BEFORE target execution.
/// `retain_lifetime` establishes an independent guardian over all descendants,
/// the fixed lease, and owner death, surviving controller loss.
///
/// `revalidate` must physically revalidate the held resources, not compare path
/// strings. `execute` must atomically fence release against its guardian, lease,
/// stop signal, and held resource identities; library polling cannot close an
/// OS-level race or interrupt a blocked callback.
///
/// Reports must describe the exact request and current bound resources.
/// Termination reports must originate from a trusted backend observation, not
/// client input. An implementation can lie; metadata equality is not proof.
pub trait Driver {
    /// Current resource metadata; must not execute or relinquish custody.
    fn binding(&self) -> Binding;
    fn prepare(&mut self, request: &Request) -> Result<Receipt, BackendError>;
    fn install_restrictions(&mut self, request: &Request) -> Result<Receipt, BackendError>;
    fn retain_lifetime(&mut self, request: &Request) -> Result<Receipt, BackendError>;
    fn revalidate(&mut self, request: &Request) -> Result<Receipt, BackendError>;
    fn execute(&mut self, request: &Request) -> Result<Receipt, BackendError>;
    fn request_cleanup(&mut self, request: &Request) -> Result<CleanupReport, BackendError>;
    fn observe_termination(&mut self, request: &Request)
        -> Result<TerminationReport, BackendError>;
}

/// Sticky, first-writer-wins notification. No threads or owner-death detection.
#[derive(Clone, Default)]
pub struct StopSignal(Arc<AtomicU8>);

impl StopSignal {
    pub fn cancel(&self) -> Result<(), Error> {
        self.set(1)
    }

    /// A caller reports loss; this does not detect loss or kill any process.
    pub fn report_owner_loss(&self) -> Result<(), Error> {
        self.set(2)
    }

    pub fn reason(&self) -> Option<Error> {
        match self.0.load(Ordering::Acquire) {
            0 => None,
            1 => Some(Error::Cancelled),
            _ => Some(Error::OwnerLost),
        }
    }

    fn set(&self, value: u8) -> Result<(), Error> {
        self.0
            .compare_exchange(0, value, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| Error::StopAlreadySignalled)
    }
}

/// Owns exactly one driver and one attempt. No driver extraction, renewal,
/// restart, external acknowledgment injection, or automatic cleanup on drop.
///
/// Dropping this value drops the driver normally; it does NOT imply termination.
/// The backend must retain independent lifetime ownership before execution.
/// Explicitly fence, request cleanup, and observe termination before dropping.
#[must_use = "drive or explicitly fence and clean up the attempt; drop is not termination"]
pub struct Controller<D, C> {
    driver: D,
    clock: C,
    binding: Binding,
    signal: StopSignal,
    status: Status,
    cleanup_release_observed: bool,
    lease: Option<Lease>,
    last_reading: Option<Reading>,
    sequence: u64,
}

impl<D: Driver, C: Clock> Controller<D, C> {
    /// Takes custody, not proof of permissions. The caller supplies independently
    /// reviewed metadata and a trusted driver; no resources are inspected here.
    pub fn new(driver: D, clock: C, binding: Binding) -> Self {
        Self {
            driver,
            clock,
            binding,
            signal: StopSignal::default(),
            status: Status {
                state: State::Ready,
                execution: Execution::NotStarted,
                cleanup: CleanupState::NotRequested,
                reason: None,
            },
            cleanup_release_observed: false,
            lease: None,
            last_reading: None,
            sequence: 0,
        }
    }

    pub fn stop_signal(&self) -> StopSignal {
        self.signal.clone()
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn lease(&self) -> Option<Lease> {
        self.lease
    }

    /// Consumes the only attempt, even on invalid input. Starts the monotonic
    /// lease, not the target. Four `advance` calls complete a healthy startup.
    pub fn start(&mut self, duration_ms: u64) -> Result<Status, Error> {
        if self.status.state != State::Ready {
            return Err(Error::AlreadyStarted);
        }
        self.status.state = State::Refused;
        if duration_ms == 0 || duration_ms > 300_000 {
            return Err(self.fence(Error::InvalidLease));
        }
        let origin = self
            .observe_control(Observation::Clock, |this| this.clock.observe())
            .map_err(|e| self.fence(Error::Clock(e)))?;
        let deadline_ms = origin
            .millis
            .checked_add(duration_ms)
            .ok_or_else(|| self.fence(Error::DeadlineOverflow))?;
        self.lease = Some(Lease {
            origin,
            deadline_ms,
        });
        self.last_reading = Some(origin);
        self.check_live()?;
        self.status.state = State::Preparing;
        Ok(self.status)
    }

    /// Executes one startup stage; the last stage revalidates then immediately
    /// hands off to execute without a caller-controlled gap. Never retries.
    pub fn advance(&mut self) -> Result<Status, Error> {
        match self.status.state {
            State::Preparing => self.run_stage(Operation::Prepare, State::Installing)?,
            State::Installing => self.run_stage(Operation::Install, State::RetainingLifetime)?,
            State::RetainingLifetime => {
                self.run_stage(Operation::RetainLifetime, State::Revalidating)?
            }
            State::Revalidating => {
                self.run_stage(Operation::Revalidate, State::Revalidating)?;
                self.run_stage(Operation::Execute, State::Running)?;
            }
            _ => return Err(Error::InvalidState),
        }
        Ok(self.status)
    }

    /// Observes fences without advancing startup. Scheduling this method is the
    /// caller's responsibility, not a kernel termination guarantee.
    pub fn poll(&mut self) -> Result<Status, Error> {
        if !self.active() {
            return Err(Error::InvalidState);
        }
        self.check_live()?;
        Ok(self.status)
    }

    /// One cleanup request maximum, including errors/unwind. Never auto-retried.
    /// A started attempt stays uncertain until `observe_termination` confirms it.
    /// Cleanup remains explicit even after confirmed natural termination.
    pub fn request_cleanup(&mut self) -> Result<Status, Error> {
        if !matches!(
            self.status.state,
            State::Refused | State::Stopping | State::Terminated
        ) {
            return Err(Error::InvalidState);
        }
        if self.status.cleanup != CleanupState::NotRequested {
            return Err(Error::CleanupAlreadyRequested);
        }
        self.status.cleanup = CleanupState::Unknown;
        let request = self.request(Operation::Cleanup)?;
        let report = self
            .driver
            .request_cleanup(&request)
            .map_err(|e| Error::Backend(Operation::Cleanup, e))?;
        Self::validate_receipt(&request, report.receipt)?;
        self.cleanup_release_observed = report.outcome == CleanupOutcome::Released;
        self.status.cleanup = match report.outcome {
            CleanupOutcome::Released
                if self.status.execution == Execution::NotStarted
                    || self.status.state == State::Terminated =>
            {
                CleanupState::Released
            }
            _ => CleanupState::Pending,
        };
        Ok(self.status)
    }

    /// Polls the uniquely owned backend, not a public event setter. A later valid
    /// observation may resolve uncertain cleanup even with a broken clock or
    /// changed current binding, but must match the ORIGINAL attempt/resources.
    pub fn observe_termination(&mut self) -> Result<Status, Error> {
        if !matches!(self.status.state, State::Running | State::Stopping) {
            return Err(Error::InvalidState);
        }
        self.observe_fences();
        let request = self.request(Operation::ObserveTermination)?;
        let prior_reason = self.status.reason;
        self.fence(Error::CallbackInterrupted(Operation::ObserveTermination));
        let result = self.driver.observe_termination(&request);
        self.status.reason = prior_reason;
        if prior_reason.is_none() {
            self.status.state = State::Running;
        }
        self.observe_fences();
        let report =
            result.map_err(|e| self.fence(Error::Backend(Operation::ObserveTermination, e)))?;
        Self::validate_receipt(&request, report.receipt).map_err(|e| self.fence(e))?;
        if report.outcome == Termination::Confirmed {
            self.status.state = State::Terminated;
            if self.cleanup_release_observed {
                self.status.cleanup = CleanupState::Released;
            }
        }
        Ok(self.status)
    }

    fn active(&self) -> bool {
        matches!(
            self.status.state,
            State::Preparing
                | State::Installing
                | State::RetainingLifetime
                | State::Revalidating
                | State::Running
        )
    }

    fn fence(&mut self, error: Error) -> Error {
        if self.status.state != State::Terminated {
            self.status.state = match self.status.execution {
                Execution::NotStarted => State::Refused,
                Execution::PossiblyStarted => State::Stopping,
            };
        }
        self.status.reason.get_or_insert(error);
        error
    }

    fn check_live(&mut self) -> Result<(), Error> {
        // Sample time after the backend binding callback, not before it: even
        // that observation can consume the remaining lease.
        if self.observe_control(Observation::Binding, |this| this.driver.binding()) != self.binding
        {
            return Err(self.fence(Error::BindingChanged));
        }
        let now = self
            .observe_control(Observation::Clock, |this| this.clock.observe())
            .map_err(|e| self.fence(Error::Clock(e)))?;
        let last = self.last_reading.ok_or(Error::InvalidState)?;
        if now.era != last.era {
            return Err(self.fence(Error::ClockChanged));
        }
        if now.millis < last.millis {
            return Err(self.fence(Error::ClockRegressed));
        }
        self.last_reading = Some(now);
        if now.millis >= self.lease.ok_or(Error::InvalidState)?.deadline_ms {
            return Err(self.fence(Error::Expired));
        }
        if let Some(error) = self.signal.reason() {
            return Err(self.fence(error));
        }
        Ok(())
    }

    fn observe_control<T>(
        &mut self,
        observation: Observation,
        read: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let prior = self.status;
        self.fence(Error::ObservationInterrupted(observation));
        let result = read(self);
        self.status = prior;
        result
    }

    fn observe_fences(&mut self) {
        // Errors are retained in status. A broken clock/stop request must not
        // prevent a trusted observation of the original worker's actual death.
        if let Err(error) = self.check_live() {
            self.fence(error);
        }
    }

    fn request(&mut self, operation: Operation) -> Result<Request, Error> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| self.fence(Error::SequenceExhausted))?;
        Ok(Request {
            receipt: Receipt {
                binding: self.binding,
                operation,
                sequence: self.sequence,
            },
            lease: self.lease,
            signal: self.signal.clone(),
        })
    }

    fn validate_receipt(request: &Request, receipt: Receipt) -> Result<(), Error> {
        if receipt == request.receipt {
            Ok(())
        } else {
            Err(Error::InvalidReceipt(request.receipt.operation))
        }
    }

    fn run_stage(&mut self, operation: Operation, next: State) -> Result<(), Error> {
        self.check_live()?;
        let request = self.request(operation)?;
        if operation == Operation::Execute {
            self.status.execution = Execution::PossiblyStarted;
        }
        // A caught backend panic must leave the attempt fenced, never retryable.
        self.fence(Error::CallbackInterrupted(operation));
        let result = match operation {
            Operation::Prepare => self.driver.prepare(&request),
            Operation::Install => self.driver.install_restrictions(&request),
            Operation::RetainLifetime => self.driver.retain_lifetime(&request),
            Operation::Revalidate => self.driver.revalidate(&request),
            Operation::Execute => self.driver.execute(&request),
            _ => unreachable!("only startup operations enter run_stage"),
        };
        self.status.reason = None;
        self.check_live()?;
        let receipt = result.map_err(|e| self.fence(Error::Backend(operation, e)))?;
        Self::validate_receipt(&request, receipt).map_err(|e| self.fence(e))?;
        self.status.state = next;
        Ok(())
    }
}
