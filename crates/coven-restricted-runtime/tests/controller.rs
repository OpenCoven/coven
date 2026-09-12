use coven_restricted_runtime::*;
use std::cell::RefCell;
use std::rc::Rc;

const BINDING: Binding = Binding {
    attempt: 1,
    backend: 2,
    worker: 3,
    workspace: 4,
    executable: 5,
    runtime_closure: 6,
    stdio_pipes: 7,
};
const STARTUP: [Operation; 5] = [
    Operation::Prepare,
    Operation::Install,
    Operation::RetainLifetime,
    Operation::Revalidate,
    Operation::Execute,
];

#[derive(Clone, Copy, Debug)]
enum Interruption {
    Cancel,
    OwnerLoss,
    Expire,
    Regress,
    ChangeClock,
    ClockFailure,
    ChangeBinding,
}

impl Interruption {
    fn error(self) -> Error {
        match self {
            Self::Cancel => Error::Cancelled,
            Self::OwnerLoss => Error::OwnerLost,
            Self::Expire => Error::Expired,
            Self::Regress => Error::ClockRegressed,
            Self::ChangeClock => Error::ClockChanged,
            Self::ClockFailure => Error::Clock(ClockError::Unavailable),
            Self::ChangeBinding => Error::BindingChanged,
        }
    }
}

const INTERRUPTIONS: [Interruption; 7] = [
    Interruption::Cancel,
    Interruption::OwnerLoss,
    Interruption::Expire,
    Interruption::Regress,
    Interruption::ChangeClock,
    Interruption::ClockFailure,
    Interruption::ChangeBinding,
];

#[derive(Clone, Copy, Debug)]
enum ReceiptFault {
    Attempt,
    Backend,
    Worker,
    Workspace,
    Executable,
    RuntimeClosure,
    StdioPipes,
    StaleSequence,
    FutureSequence,
    WrongPhase,
}

const RECEIPT_FAULTS: [ReceiptFault; 10] = [
    ReceiptFault::Attempt,
    ReceiptFault::Backend,
    ReceiptFault::Worker,
    ReceiptFault::Workspace,
    ReceiptFault::Executable,
    ReceiptFault::RuntimeClosure,
    ReceiptFault::StdioPipes,
    ReceiptFault::StaleSequence,
    ReceiptFault::FutureSequence,
    ReceiptFault::WrongPhase,
];

struct Fixture {
    clock: Result<Reading, ClockError>,
    binding: Binding,
    binding_checks: usize,
    binding_interrupt: Option<(usize, Interruption)>,
    calls: Vec<Request>,
    fail: Option<Operation>,
    interrupt: Option<(Operation, Interruption)>,
    receipt_fault: Option<(Operation, ReceiptFault)>,
    panic_at: Option<Operation>,
    panic_binding: bool,
    panic_clock: bool,
    signal: Option<StopSignal>,
    cleanup: CleanupOutcome,
    termination: Termination,
    executions: usize,
    drops: usize,
}

impl Fixture {
    fn interrupt(&mut self, interruption: Interruption) {
        match interruption {
            Interruption::Cancel => self.signal.as_ref().unwrap().cancel().unwrap(),
            Interruption::OwnerLoss => self.signal.as_ref().unwrap().report_owner_loss().unwrap(),
            Interruption::Expire => self.clock.as_mut().unwrap().millis = 1_100,
            Interruption::Regress => self.clock.as_mut().unwrap().millis = 99,
            Interruption::ChangeClock => self.clock.as_mut().unwrap().era += 1,
            Interruption::ClockFailure => self.clock = Err(ClockError::Unavailable),
            Interruption::ChangeBinding => self.binding.worker += 1,
        }
    }

    fn operations(&self) -> Vec<Operation> {
        self.calls.iter().map(|r| r.receipt.operation).collect()
    }
}

struct FakeDriver(Rc<RefCell<Fixture>>);
struct FakeClock(Rc<RefCell<Fixture>>);

impl Clock for FakeClock {
    fn observe(&mut self) -> Result<Reading, ClockError> {
        assert!(!self.0.borrow().panic_clock, "fixture clock panic");
        self.0.borrow().clock
    }
}

impl FakeDriver {
    fn perform(&mut self, request: &Request) -> Result<Receipt, BackendError> {
        let mut f = self.0.borrow_mut();
        let operation = request.receipt.operation;
        f.calls.push(request.clone());
        assert_eq!(request.initial_environment(), &[]);
        assert_eq!(request.stdio_pipe_identity(), BINDING.stdio_pipes);
        if STARTUP.contains(&operation) {
            assert!(request.lease.is_some());
            assert_eq!(request.stop_signal().reason(), None);
        }
        if operation == Operation::Execute {
            // This fake effect intentionally occurs before an execute error.
            f.executions += 1;
        }
        if let Some((at, interruption)) = f.interrupt {
            if at == operation {
                f.interrupt(interruption);
            }
        }
        assert_ne!(f.panic_at, Some(operation), "fixture callback panic");
        if f.fail == Some(operation) {
            return Err(BackendError::Unavailable);
        }
        let mut receipt = request.receipt;
        if let Some((at, fault)) = f.receipt_fault {
            if at == operation {
                match fault {
                    ReceiptFault::Attempt => receipt.binding.attempt += 1,
                    ReceiptFault::Backend => receipt.binding.backend += 1,
                    ReceiptFault::Worker => receipt.binding.worker += 1,
                    ReceiptFault::Workspace => receipt.binding.workspace += 1,
                    ReceiptFault::Executable => receipt.binding.executable += 1,
                    ReceiptFault::RuntimeClosure => receipt.binding.runtime_closure += 1,
                    ReceiptFault::StdioPipes => receipt.binding.stdio_pipes += 1,
                    ReceiptFault::StaleSequence => receipt.sequence -= 1,
                    ReceiptFault::FutureSequence => receipt.sequence += 1,
                    ReceiptFault::WrongPhase => {
                        receipt.operation = if operation == Operation::ObserveTermination {
                            Operation::Execute
                        } else {
                            Operation::ObserveTermination
                        };
                    }
                }
            }
        }
        Ok(receipt)
    }
}

impl Drop for FakeDriver {
    fn drop(&mut self) {
        self.0.borrow_mut().drops += 1;
    }
}

impl Driver for FakeDriver {
    fn binding(&self) -> Binding {
        let mut f = self.0.borrow_mut();
        assert!(!f.panic_binding, "fixture binding panic");
        f.binding_checks += 1;
        if let Some((at, interruption)) = f.binding_interrupt {
            if at == f.binding_checks {
                f.interrupt(interruption);
            }
        }
        f.binding
    }

    fn prepare(&mut self, request: &Request) -> Result<Receipt, BackendError> {
        self.perform(request)
    }

    fn install_restrictions(&mut self, request: &Request) -> Result<Receipt, BackendError> {
        self.perform(request)
    }

    fn retain_lifetime(&mut self, request: &Request) -> Result<Receipt, BackendError> {
        self.perform(request)
    }

    fn revalidate(&mut self, request: &Request) -> Result<Receipt, BackendError> {
        self.perform(request)
    }

    fn execute(&mut self, request: &Request) -> Result<Receipt, BackendError> {
        self.perform(request)
    }

    fn request_cleanup(&mut self, request: &Request) -> Result<CleanupReport, BackendError> {
        let receipt = self.perform(request)?;
        Ok(CleanupReport {
            receipt,
            outcome: self.0.borrow().cleanup,
        })
    }

    fn observe_termination(
        &mut self,
        request: &Request,
    ) -> Result<TerminationReport, BackendError> {
        let receipt = self.perform(request)?;
        Ok(TerminationReport {
            receipt,
            outcome: self.0.borrow().termination,
        })
    }
}

type TestController = Controller<FakeDriver, FakeClock>;

fn fixture() -> (TestController, Rc<RefCell<Fixture>>) {
    let f = Rc::new(RefCell::new(Fixture {
        clock: Ok(Reading {
            era: 9,
            millis: 100,
        }),
        binding: BINDING,
        binding_checks: 0,
        binding_interrupt: None,
        calls: Vec::new(),
        fail: None,
        interrupt: None,
        receipt_fault: None,
        panic_at: None,
        panic_binding: false,
        panic_clock: false,
        signal: None,
        cleanup: CleanupOutcome::Released,
        termination: Termination::Pending,
        executions: 0,
        drops: 0,
    }));
    let controller = Controller::new(FakeDriver(f.clone()), FakeClock(f.clone()), BINDING);
    f.borrow_mut().signal = Some(controller.stop_signal());
    (controller, f)
}

fn launch(c: &mut TestController) -> Result<Status, Error> {
    c.start(1_000)?;
    for _ in 0..4 {
        c.advance()?;
    }
    Ok(c.status())
}

fn assert_fenced(c: &mut TestController, f: &Rc<RefCell<Fixture>>, reason: Error, started: bool) {
    let status = c.status();
    assert_eq!(status.reason, Some(reason));
    assert_eq!(
        status.state,
        if started {
            State::Stopping
        } else {
            State::Refused
        }
    );
    assert_eq!(
        status.execution,
        if started {
            Execution::PossiblyStarted
        } else {
            Execution::NotStarted
        }
    );
    let calls = f.borrow().calls.len();
    assert_eq!(c.start(1_000), Err(Error::AlreadyStarted));
    assert_eq!(c.advance(), Err(Error::InvalidState));
    assert_eq!(f.borrow().calls.len(), calls);
    assert_eq!(f.borrow().drops, 0, "driver custody must survive errors");
}

#[test]
fn healthy_fake_flow_orders_real_controller_transitions_and_fixed_contract() {
    let (mut c, f) = fixture();
    assert_eq!(c.start(1_000).unwrap().state, State::Preparing);
    for expected in [
        State::Installing,
        State::RetainingLifetime,
        State::Revalidating,
        State::Running,
    ] {
        assert_eq!(c.advance().unwrap().state, expected);
    }
    assert_eq!(f.borrow().operations(), STARTUP);
    assert_eq!(f.borrow().executions, 1);
    assert_eq!(c.status().execution, Execution::PossiblyStarted);
    let lease = c.lease().unwrap();
    assert_eq!(
        lease.origin,
        Reading {
            era: 9,
            millis: 100
        }
    );
    assert_eq!(lease.deadline_ms, 1_100);
    for (i, request) in f.borrow().calls.iter().enumerate() {
        assert_eq!(request.receipt.sequence, i as u64 + 1);
        assert_eq!(request.receipt.binding, BINDING);
        assert_eq!(request.lease, Some(lease));
    }
    assert_eq!(c.start(1), Err(Error::AlreadyStarted));
    assert_eq!(c.advance(), Err(Error::InvalidState));
    assert_eq!(c.request_cleanup(), Err(Error::InvalidState));
    assert_eq!(c.poll().unwrap().state, State::Running);
    f.borrow_mut().termination = Termination::Confirmed;
    assert_eq!(c.observe_termination().unwrap().state, State::Terminated);
    assert_eq!(c.status().execution, Execution::PossiblyStarted);
    assert_eq!(c.observe_termination(), Err(Error::InvalidState));
    assert_eq!(c.start(1_000), Err(Error::AlreadyStarted));
    assert_eq!(f.borrow().executions, 1);
}

#[test]
fn every_startup_failure_fences_later_stages_and_retains_cleanup_custody() {
    for (index, operation) in STARTUP.into_iter().enumerate() {
        let (mut c, f) = fixture();
        f.borrow_mut().fail = Some(operation);
        let error = Error::Backend(operation, BackendError::Unavailable);
        assert_eq!(launch(&mut c), Err(error), "{operation:?}");
        assert_eq!(f.borrow().operations(), STARTUP[..=index]);
        assert_fenced(&mut c, &f, error, operation == Operation::Execute);
        c.request_cleanup().unwrap();
        assert_ne!(c.status().state, State::Terminated);
        assert_eq!(c.request_cleanup(), Err(Error::CleanupAlreadyRequested));
        assert_eq!(f.borrow().operations().last(), Some(&Operation::Cleanup));
    }
}

#[test]
fn each_callback_is_followed_by_a_clock_signal_and_binding_fence() {
    for (index, operation) in STARTUP.into_iter().enumerate() {
        for interruption in INTERRUPTIONS {
            let (mut c, f) = fixture();
            f.borrow_mut().interrupt = Some((operation, interruption));
            assert_eq!(
                launch(&mut c),
                Err(interruption.error()),
                "{operation:?} {interruption:?}"
            );
            assert_eq!(f.borrow().operations(), STARTUP[..=index]);
            assert_fenced(
                &mut c,
                &f,
                interruption.error(),
                operation == Operation::Execute,
            );
        }
    }
}

#[test]
fn interruptions_between_all_public_startup_steps_prevent_the_next_callback() {
    for steps in 0..=4 {
        for interruption in INTERRUPTIONS {
            let (mut c, f) = fixture();
            c.start(1_000).unwrap();
            for _ in 0..steps {
                c.advance().unwrap();
            }
            let calls = f.borrow().calls.len();
            f.borrow_mut().interrupt(interruption);
            assert_eq!(c.poll(), Err(interruption.error()));
            assert_fenced(&mut c, &f, interruption.error(), steps == 4);
            assert_eq!(f.borrow().calls.len(), calls);
        }
    }
}

#[test]
fn advance_itself_checks_fences_without_a_separate_poll() {
    for steps in 0..4 {
        for interruption in INTERRUPTIONS {
            let (mut c, f) = fixture();
            c.start(1_000).unwrap();
            for _ in 0..steps {
                c.advance().unwrap();
            }
            let calls = f.borrow().calls.len();
            f.borrow_mut().interrupt(interruption);
            assert_eq!(c.advance(), Err(interruption.error()));
            assert_eq!(f.borrow().calls.len(), calls);
        }
    }
}

#[test]
fn startup_reports_cannot_skip_phases_or_substitute_any_reviewed_identity() {
    for operation in STARTUP {
        for fault in RECEIPT_FAULTS {
            let (mut c, f) = fixture();
            f.borrow_mut().receipt_fault = Some((operation, fault));
            let error = Error::InvalidReceipt(operation);
            assert_eq!(launch(&mut c), Err(error), "{operation:?} {fault:?}");
            assert_fenced(&mut c, &f, error, operation == Operation::Execute);
        }
    }
}

#[test]
fn invalid_lease_consumes_the_attempt_without_preparing() {
    for duration in [0, 300_001, u64::MAX] {
        let (mut c, f) = fixture();
        assert_eq!(c.start(duration), Err(Error::InvalidLease));
        assert_fenced(&mut c, &f, Error::InvalidLease, false);
        assert!(f.borrow().calls.is_empty());
    }
}

#[test]
fn maximum_lease_is_nonrenewable_and_fences_at_the_exact_deadline() {
    let (mut c, f) = fixture();
    c.start(300_000).unwrap();
    let lease = c.lease().unwrap();
    f.borrow_mut().clock.as_mut().unwrap().millis = 300_099;
    assert_eq!(c.poll().unwrap().state, State::Preparing);
    assert_eq!(c.start(300_000), Err(Error::AlreadyStarted));
    assert_eq!(c.lease(), Some(lease));
    f.borrow_mut().clock.as_mut().unwrap().millis = 300_100;
    assert_eq!(c.advance(), Err(Error::Expired));
    assert!(f.borrow().calls.is_empty());
}

#[test]
fn lease_deadline_overflow_and_initial_clock_failure_refuse_without_callbacks() {
    let (mut c, f) = fixture();
    f.borrow_mut().clock.as_mut().unwrap().millis = u64::MAX;
    assert_eq!(c.start(1), Err(Error::DeadlineOverflow));
    assert_fenced(&mut c, &f, Error::DeadlineOverflow, false);
    assert!(f.borrow().calls.is_empty());
    let (mut c, f) = fixture();
    f.borrow_mut().clock = Err(ClockError::Unavailable);
    assert_eq!(c.start(1), Err(Error::Clock(ClockError::Unavailable)));
    assert!(f.borrow().calls.is_empty());
}

#[test]
fn regression_is_relative_to_last_observation_not_only_lease_origin() {
    let (mut c, f) = fixture();
    c.start(1_000).unwrap();
    f.borrow_mut().clock.as_mut().unwrap().millis = 200;
    c.advance().unwrap();
    f.borrow_mut().clock.as_mut().unwrap().millis = 199;
    assert_eq!(c.advance(), Err(Error::ClockRegressed));
    assert_eq!(f.borrow().operations(), [Operation::Prepare]);
}

#[test]
fn signals_are_sticky_before_start_and_duplicate_signals_are_errors() {
    for owner_loss in [false, true] {
        let (mut c, f) = fixture();
        let signal = c.stop_signal();
        let error = if owner_loss {
            signal.report_owner_loss().unwrap();
            Error::OwnerLost
        } else {
            signal.cancel().unwrap();
            Error::Cancelled
        };
        assert_eq!(signal.reason(), Some(error));
        assert_eq!(signal.cancel(), Err(Error::StopAlreadySignalled));
        assert_eq!(signal.report_owner_loss(), Err(Error::StopAlreadySignalled));
        assert_eq!(c.start(1_000), Err(error));
        assert_fenced(&mut c, &f, error, false);
        assert!(f.borrow().calls.is_empty());
    }
}

#[test]
fn initial_binding_mismatch_refuses_without_preparation() {
    let (mut c, f) = fixture();
    f.borrow_mut().binding.workspace += 1;
    assert_eq!(c.start(1_000), Err(Error::BindingChanged));
    assert!(f.borrow().calls.is_empty());
}

#[test]
fn execute_error_is_never_not_started_even_after_successful_cleanup() {
    let (mut c, f) = fixture();
    f.borrow_mut().fail = Some(Operation::Execute);
    let error = Error::Backend(Operation::Execute, BackendError::Unavailable);
    assert_eq!(launch(&mut c), Err(error));
    assert_eq!(f.borrow().executions, 1);
    c.request_cleanup().unwrap();
    assert_eq!(c.status().cleanup, CleanupState::Pending);
    assert_fenced(&mut c, &f, error, true);
    assert_eq!(c.observe_termination().unwrap().state, State::Stopping);
    f.borrow_mut().termination = Termination::Confirmed;
    assert_eq!(c.observe_termination().unwrap().state, State::Terminated);
    assert_eq!(c.status().execution, Execution::PossiblyStarted);
    assert_eq!(c.status().reason, Some(error));
    assert_eq!(f.borrow().executions, 1);
}

#[test]
fn confirmed_termination_combines_with_prior_release_but_does_not_invent_it() {
    for outcome in [CleanupOutcome::Pending, CleanupOutcome::Released] {
        let (mut c, f) = fixture();
        launch(&mut c).unwrap();
        c.stop_signal().cancel().unwrap();
        assert_eq!(c.poll(), Err(Error::Cancelled));
        f.borrow_mut().cleanup = outcome;
        c.request_cleanup().unwrap();
        assert_eq!(c.status().cleanup, CleanupState::Pending);
        f.borrow_mut().termination = Termination::Confirmed;
        let status = c.observe_termination().unwrap();
        assert_eq!(status.state, State::Terminated);
        assert_eq!(
            status.cleanup,
            if outcome == CleanupOutcome::Released {
                CleanupState::Released
            } else {
                CleanupState::Pending
            }
        );
        assert_eq!(status.execution, Execution::PossiblyStarted);
        assert_eq!(status.reason, Some(Error::Cancelled));
        assert_eq!(
            f.borrow()
                .operations()
                .iter()
                .filter(|op| **op == Operation::Cleanup)
                .count(),
            1
        );
    }
}

#[test]
fn failed_stop_can_be_resolved_only_by_a_later_matching_terminal_observation() {
    let (mut c, f) = fixture();
    launch(&mut c).unwrap();
    c.stop_signal().cancel().unwrap();
    assert_eq!(c.poll(), Err(Error::Cancelled));
    f.borrow_mut().fail = Some(Operation::Cleanup);
    assert_eq!(
        c.request_cleanup(),
        Err(Error::Backend(
            Operation::Cleanup,
            BackendError::Unavailable
        ))
    );
    assert_eq!(c.status().cleanup, CleanupState::Unknown);
    assert_eq!(c.status().state, State::Stopping);
    assert_eq!(c.request_cleanup(), Err(Error::CleanupAlreadyRequested));
    f.borrow_mut().fail = Some(Operation::ObserveTermination);
    assert_eq!(
        c.observe_termination(),
        Err(Error::Backend(
            Operation::ObserveTermination,
            BackendError::Unavailable
        ))
    );
    f.borrow_mut().fail = None;
    assert_eq!(c.observe_termination().unwrap().state, State::Stopping);
    f.borrow_mut().termination = Termination::Confirmed;
    assert_eq!(c.observe_termination().unwrap().state, State::Terminated);
    assert_eq!(c.status().cleanup, CleanupState::Unknown);
    assert_eq!(c.status().reason, Some(Error::Cancelled));
    assert_eq!(c.status().execution, Execution::PossiblyStarted);
}

#[test]
fn stale_or_mismatched_terminal_reports_do_not_terminate_and_later_valid_reports_can() {
    for fault in RECEIPT_FAULTS {
        let (mut c, f) = fixture();
        launch(&mut c).unwrap();
        f.borrow_mut().termination = Termination::Confirmed;
        f.borrow_mut().receipt_fault = Some((Operation::ObserveTermination, fault));
        assert_eq!(
            c.observe_termination(),
            Err(Error::InvalidReceipt(Operation::ObserveTermination))
        );
        assert_eq!(c.status().state, State::Stopping);
        assert_eq!(c.start(1_000), Err(Error::AlreadyStarted));
        f.borrow_mut().receipt_fault = None;
        assert_eq!(c.observe_termination().unwrap().state, State::Terminated);
        assert_eq!(f.borrow().executions, 1);
    }
}

#[test]
fn pre_execution_cleanup_releases_resources_but_never_becomes_termination() {
    for outcome in [CleanupOutcome::Pending, CleanupOutcome::Released] {
        let (mut c, f) = fixture();
        f.borrow_mut().fail = Some(Operation::Install);
        f.borrow_mut().cleanup = outcome;
        assert!(launch(&mut c).is_err());
        assert_eq!(c.observe_termination(), Err(Error::InvalidState));
        c.request_cleanup().unwrap();
        assert_eq!(c.status().state, State::Refused);
        assert_eq!(c.status().execution, Execution::NotStarted);
        assert_eq!(
            c.status().cleanup,
            if outcome == CleanupOutcome::Released {
                CleanupState::Released
            } else {
                CleanupState::Pending
            }
        );
        assert_eq!(c.request_cleanup(), Err(Error::CleanupAlreadyRequested));
    }
}

#[test]
fn bad_cleanup_acknowledgments_cannot_claim_released_or_terminated() {
    for started in [false, true] {
        for fault in RECEIPT_FAULTS {
            let (mut c, f) = fixture();
            if started {
                launch(&mut c).unwrap();
                c.stop_signal().cancel().unwrap();
                assert_eq!(c.poll(), Err(Error::Cancelled));
            } else {
                f.borrow_mut().fail = Some(Operation::Prepare);
                assert!(launch(&mut c).is_err());
            }
            f.borrow_mut().receipt_fault = Some((Operation::Cleanup, fault));
            assert_eq!(
                c.request_cleanup(),
                Err(Error::InvalidReceipt(Operation::Cleanup))
            );
            assert_eq!(c.status().cleanup, CleanupState::Unknown);
            assert_ne!(c.status().state, State::Terminated);
            assert_eq!(c.request_cleanup(), Err(Error::CleanupAlreadyRequested));
        }
    }
}

#[test]
fn terminal_observation_remains_available_after_clock_failure_or_changed_binding() {
    for interruption in INTERRUPTIONS {
        let (mut c, f) = fixture();
        launch(&mut c).unwrap();
        f.borrow_mut().interrupt(interruption);
        assert_eq!(c.poll(), Err(interruption.error()));
        f.borrow_mut().termination = Termination::Confirmed;
        assert_eq!(c.observe_termination().unwrap().state, State::Terminated);
        assert_eq!(c.status().reason, Some(interruption.error()));
    }
}

#[test]
fn terminal_poll_checks_interruptions_after_its_callback_without_masking_confirmed_death() {
    for interruption in INTERRUPTIONS {
        for confirmed in [false, true] {
            let (mut c, f) = fixture();
            launch(&mut c).unwrap();
            f.borrow_mut().interrupt = Some((Operation::ObserveTermination, interruption));
            f.borrow_mut().termination = if confirmed {
                Termination::Confirmed
            } else {
                Termination::Pending
            };
            let status = c.observe_termination().unwrap();
            assert_eq!(
                status.state,
                if confirmed {
                    State::Terminated
                } else {
                    State::Stopping
                }
            );
            assert_eq!(status.reason, Some(interruption.error()));
        }
    }
}

#[test]
fn invalid_calls_do_not_call_the_driver_and_terminal_cleanup_is_explicit() {
    let (mut c, f) = fixture();
    assert_eq!(c.advance(), Err(Error::InvalidState));
    assert_eq!(c.poll(), Err(Error::InvalidState));
    assert_eq!(c.request_cleanup(), Err(Error::InvalidState));
    assert_eq!(c.observe_termination(), Err(Error::InvalidState));
    assert!(f.borrow().calls.is_empty());
    launch(&mut c).unwrap();
    f.borrow_mut().termination = Termination::Confirmed;
    c.observe_termination().unwrap();
    assert_eq!(c.poll(), Err(Error::InvalidState));
    c.request_cleanup().unwrap();
    assert_eq!(c.status().state, State::Terminated);
}

#[test]
fn callback_unwind_fences_retry_and_preserves_handoff_evidence() {
    for operation in STARTUP {
        let (mut c, f) = fixture();
        f.borrow_mut().panic_at = Some(operation);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| launch(&mut c)));
        assert!(result.is_err());
        assert_fenced(
            &mut c,
            &f,
            Error::CallbackInterrupted(operation),
            operation == Operation::Execute,
        );
        f.borrow_mut().panic_at = None;
        c.request_cleanup().unwrap();
    }
}

fn observation_unwind_cannot_resume(panic_binding: bool, started: bool) {
    let (mut c, f) = fixture();
    if started {
        launch(&mut c).unwrap();
    } else {
        c.start(1_000).unwrap();
        for _ in 0..3 {
            c.advance().unwrap();
        }
    }
    f.borrow_mut().panic_binding = panic_binding;
    f.borrow_mut().panic_clock = !panic_binding;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if started {
            c.poll()
        } else {
            c.advance()
        }
    }));
    assert!(result.is_err());
    f.borrow_mut().panic_binding = false;
    f.borrow_mut().panic_clock = false;
    assert_eq!(
        c.status().state,
        if started {
            State::Stopping
        } else {
            State::Refused
        }
    );
    assert!(c.status().reason.is_some());
    assert_eq!(c.advance(), Err(Error::InvalidState));
    assert_eq!(f.borrow().executions, usize::from(started));
    c.request_cleanup().unwrap();
}

#[test]
fn binding_observation_unwind_fences_startup_and_running_attempts() {
    for started in [false, true] {
        observation_unwind_cannot_resume(true, started);
    }
}

#[test]
fn clock_observation_unwind_fences_startup_and_running_attempts() {
    for started in [false, true] {
        observation_unwind_cannot_resume(false, started);
    }
}

#[test]
fn drop_has_no_hidden_cleanup_or_termination_callback() {
    let (mut c, f) = fixture();
    launch(&mut c).unwrap();
    drop(c);
    assert_eq!(f.borrow().operations(), STARTUP);
    assert_eq!(f.borrow().executions, 1);
    assert_eq!(f.borrow().drops, 1);
}

#[test]
fn metadata_debug_is_redacted_and_public_outcomes_are_bounded_codes() {
    assert_eq!(format!("{BINDING:?}"), "Binding { .. }");
    let (mut c, _) = fixture();
    c.start(0).unwrap_err();
    assert_eq!(c.status().reason, Some(Error::InvalidLease));
    assert!(format!("{:?}", c.status()).contains("InvalidLease"));
}

#[test]
fn retained_backend_request_receives_later_stop_signals_without_controller_polling() {
    let (mut c, f) = fixture();
    launch(&mut c).unwrap();
    c.stop_signal().report_owner_loss().unwrap();
    assert_eq!(
        f.borrow().calls[2].stop_signal().reason(),
        Some(Error::OwnerLost)
    );
    // Delivery is not a claim that a guardian or OS termination exists.
    assert_eq!(c.status().state, State::Running);
}

#[test]
fn binding_observation_latency_is_checked_before_each_startup_operation() {
    for (index, operation) in STARTUP.into_iter().enumerate() {
        for interruption in INTERRUPTIONS {
            let (mut c, f) = fixture();
            // Each operation has a pre/post binding observation; start has one.
            f.borrow_mut().binding_interrupt = Some((2 + index * 2, interruption));
            assert_eq!(launch(&mut c), Err(interruption.error()));
            assert_eq!(
                f.borrow().operations(),
                STARTUP[..index],
                "{operation:?} must not run after {interruption:?} during its binding read"
            );
            assert_fenced(&mut c, &f, interruption.error(), false);
        }
    }
}

#[test]
fn interruption_during_initial_binding_observation_refuses_start() {
    for interruption in INTERRUPTIONS {
        let (mut c, f) = fixture();
        f.borrow_mut().binding_interrupt = Some((1, interruption));
        assert_eq!(c.start(1_000), Err(interruption.error()));
        assert_fenced(&mut c, &f, interruption.error(), false);
        assert!(f.borrow().calls.is_empty());
    }
}
