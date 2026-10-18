//! Bounded unit-test observations; no provider values, paths or raw errors.
use std::fmt::Write as _;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Instant;

use super::StartupReconciliationWakeScenario;

const MAX_RECORDS: usize = 64;

macro_rules! labels {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        #[derive(Clone, Copy)]
        pub(super) enum $name { $($variant),+ }
        impl $name {
            fn label(self) -> &'static str {
                match self { $(Self::$variant => $label),+ }
            }
        }
    };
}

labels!(StartupWakePhase {
    TempHome => "temp-home",
    PrivateHome => "private-home",
    StoreOpenInitial => "store-open-initial",
    DefinitionInsert => "definition-insert",
    DefinitionTimeUpdate => "definition-time-update",
    SchedulerStart => "scheduler-start",
    SharedDeadline => "shared-deadline-created",
    SystemWaitReceive => "system-wait-receive",
    LaunchCountBeforeWait => "launch-count-before-wait",
    GateRelease => "gate-release",
    Action => "definition-action",
    ActionOutcome => "action-outcome",
    LaunchReceive => "launch-receive",
    FirstJoin => "first-join",
    CleanupDeadline => "cleanup-deadline-created",
    CleanupJoin => "cleanup-join",
    RetainHome => "retain-home",
    StoreOpenFinal => "store-open-final",
    OccurrenceQuery => "occurrence-query",
    LastPassQuery => "last-pass-query",
    DefinitionReadback => "definition-readback",
    OccurrencePresent => "occurrence-present",
    LastPassPresent => "last-pass-present",
    DefinitionPresent => "definition-present",
    Comparison => "comparison-reached",
});

labels!(StartupWakeOutcome {
    Begin => "begin",
    Ok => "ok",
    Error => "error",
    Received => "received",
    Timeout => "timeout",
    Disconnected => "disconnected",
    TransportError => "transport-error",
    Non200 => "non-200",
    InvalidBody => "invalid-body",
    NotAccepted => "not-accepted",
    Accepted => "accepted",
    DeadlineExpired => "deadline-expired",
    WorkerPanic => "worker-panic",
    OtherJoinError => "other-join-error",
});

struct Record {
    phase: StartupWakePhase,
    outcome: StartupWakeOutcome,
    elapsed_us: u128,
    remaining_us: Option<u128>,
    value: Option<u64>,
}

pub(super) struct StartupWakeTrace {
    started: Instant,
    scenario: &'static str,
    deadline: Option<Instant>,
    records: Vec<Record>,
    overflow: bool,
}

impl StartupWakeTrace {
    pub(super) fn new(scenario: StartupReconciliationWakeScenario) -> Self {
        Self {
            started: Instant::now(),
            scenario: match scenario {
                StartupReconciliationWakeScenario::StartupImmediateReconcile => "startup",
                StartupReconciliationWakeScenario::DefinitionChangeWake => "definition-change",
            },
            deadline: None,
            records: Vec::with_capacity(MAX_RECORDS),
            overflow: false,
        }
    }

    fn record(&mut self, phase: StartupWakePhase, outcome: StartupWakeOutcome, value: Option<u64>) {
        if self.records.len() == MAX_RECORDS {
            self.overflow = true;
            return;
        }
        let now = Instant::now();
        self.records.push(Record {
            phase,
            outcome,
            elapsed_us: now.duration_since(self.started).as_micros(),
            remaining_us: self
                .deadline
                .map(|deadline| deadline.saturating_duration_since(now).as_micros()),
            value,
        });
    }

    pub(super) fn event(&mut self, phase: StartupWakePhase, outcome: StartupWakeOutcome) {
        self.record(phase, outcome, None);
    }

    pub(super) fn value(&mut self, phase: StartupWakePhase, value: u64) {
        self.record(phase, StartupWakeOutcome::Ok, Some(value));
    }

    pub(super) fn set_deadline(&mut self, phase: StartupWakePhase, deadline: Instant) {
        self.deadline = Some(deadline);
        self.event(phase, StartupWakeOutcome::Ok);
    }

    pub(super) fn operation<T, E>(
        &mut self,
        phase: StartupWakePhase,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        self.event(phase, StartupWakeOutcome::Begin);
        let result = operation();
        self.event(
            phase,
            if result.is_ok() {
                StartupWakeOutcome::Ok
            } else {
                StartupWakeOutcome::Error
            },
        );
        result
    }

    pub(super) fn receive<T>(
        &mut self,
        phase: StartupWakePhase,
        operation: impl FnOnce() -> Result<T, RecvTimeoutError>,
    ) -> Result<T, RecvTimeoutError> {
        self.event(phase, StartupWakeOutcome::Begin);
        let result = operation();
        self.event(
            phase,
            match &result {
                Ok(_) => StartupWakeOutcome::Received,
                Err(RecvTimeoutError::Timeout) => StartupWakeOutcome::Timeout,
                Err(RecvTimeoutError::Disconnected) => StartupWakeOutcome::Disconnected,
            },
        );
        result
    }

    pub(super) fn join(
        &mut self,
        phase: StartupWakePhase,
        operation: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.event(phase, StartupWakeOutcome::Begin);
        let result = operation();
        let outcome = match &result {
            Ok(()) => StartupWakeOutcome::Ok,
            // These are the helper's own fixed internal errors. Inspect them
            // only to classify; arbitrary error messages never enter a record.
            Err(error) => match error.to_string().as_str() {
                "automations scheduler did not stop before the conformance deadline" => {
                    StartupWakeOutcome::DeadlineExpired
                }
                "automations scheduler thread panicked during shutdown" => {
                    StartupWakeOutcome::WorkerPanic
                }
                _ => StartupWakeOutcome::OtherJoinError,
            },
        };
        self.event(phase, outcome);
        result
    }

    pub(super) fn failure_report(&self, result: &Result<bool, &'static str>) -> Option<String> {
        if !matches!(result, Err("conformance suite execution failed")) {
            return None;
        }
        let mut report = format!(
            "startup-reconciliation-trace scenario={} overflow={}",
            self.scenario, self.overflow
        );
        for record in &self.records {
            let _ = write!(
                report,
                "\nphase={} outcome={} elapsed_us={} remaining_us={:?} value={:?}",
                record.phase.label(),
                record.outcome.label(),
                record.elapsed_us,
                record.remaining_us,
                record.value
            );
        }
        Some(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    fn trace() -> StartupWakeTrace {
        StartupWakeTrace::new(StartupReconciliationWakeScenario::StartupImmediateReconcile)
    }

    #[test]
    fn real_receives_distinguish_timeout_disconnection_and_delivery() {
        let mut trace = trace();
        let (tx, rx) = channel();
        assert_eq!(
            trace.receive(StartupWakePhase::SystemWaitReceive, || rx
                .recv_timeout(Duration::ZERO)),
            Err(RecvTimeoutError::Timeout)
        );
        tx.send(()).unwrap();
        assert_eq!(
            trace.receive(StartupWakePhase::SystemWaitReceive, || rx
                .recv_timeout(Duration::ZERO)),
            Ok(())
        );
        drop(tx);
        assert_eq!(
            trace.receive(StartupWakePhase::LaunchReceive, || rx
                .recv_timeout(Duration::ZERO)),
            Err(RecvTimeoutError::Disconnected)
        );
        let report = trace
            .failure_report(&Err("conformance suite execution failed"))
            .unwrap();
        for expected in [
            "outcome=timeout",
            "outcome=received",
            "outcome=disconnected",
        ] {
            assert!(report.contains(expected), "{report}");
        }
    }

    #[test]
    fn errors_keep_their_value_without_entering_the_report() {
        let mut trace = trace();
        let result = trace.operation(StartupWakePhase::StoreOpenInitial, || {
            Err::<(), _>(std::io::Error::other(
                "private sentinel path and provider input",
            ))
        });
        assert_eq!(
            result.unwrap_err().to_string(),
            "private sentinel path and provider input"
        );
        let report = trace
            .failure_report(&Err("conformance suite execution failed"))
            .unwrap();
        assert!(report.contains("phase=store-open-initial outcome=error"));
        assert!(!report.contains("private"));
        for result in [Ok(true), Ok(false), Err("conformance vector is invalid")] {
            assert!(trace.failure_report(&result).is_none());
        }
    }

    #[test]
    fn helper_identifies_actual_store_open_failure_without_private_path() {
        let vectors: super::super::StartupReconciliationWakeVectorSet = serde_json::from_str(
            include_str!("../../../../../conformance/automations/runner/startup-reconciliation-wake.vectors.json"),
        )
        .unwrap();
        let home = tempfile::Builder::new()
            .prefix("private-store-diagnostic-")
            .tempdir()
            .unwrap();
        // A directory at the database path fails Coven's non-regular-file
        // validation in open_store, before SQLite or a scheduler is started.
        std::fs::create_dir(home.path().join("coven.sqlite3")).unwrap();
        let private_path = home.path().to_string_lossy().into_owned();
        let mut trace = trace();
        let result = super::super::startup_reconciliation_wake_case_in_home(
            &vectors.cases[0],
            home,
            &mut trace,
        );
        assert_eq!(result, Err("conformance suite execution failed"));
        let report = trace.failure_report(&result).unwrap();
        assert!(
            report.contains("phase=store-open-initial outcome=error"),
            "missing owning-operation diagnostic: {report}"
        );
        assert!(!report.contains("phase=scheduler-start"), "{report}");
        assert!(!report.contains(&private_path), "private home leaked");
        assert!(!report.contains("coven.sqlite3"), "database path leaked");
    }

    #[test]
    fn real_first_join_failure_keeps_generic_error_after_successful_cleanup() {
        use crate::automations::daemon_tick::{
            AutomationClock, AutomationSchedulerHandle, AutomationWakeSignal, MonotonicInstant,
            SystemAutomationClock, WakeReason,
        };
        use std::sync::{mpsc::sync_channel, Arc, Mutex};

        struct ControlledClock {
            system: SystemAutomationClock,
            ready: std::sync::mpsc::SyncSender<()>,
            release: Mutex<std::sync::mpsc::Receiver<()>>,
        }
        impl AutomationClock for ControlledClock {
            fn now_utc(&self) -> chrono::DateTime<chrono::Utc> {
                chrono::DateTime::parse_from_rfc3339("2099-09-03T12:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc)
            }
            fn monotonic_now(&self) -> MonotonicInstant {
                self.system.monotonic_now()
            }
            fn sleep_until_or_wake(
                &self,
                _deadline: MonotonicInstant,
                _wake: &AutomationWakeSignal,
                _generation: u64,
            ) -> WakeReason {
                // This gate owns no production wake mutex. Shutdown can set
                // its signal while the actual scheduler worker stays blocked.
                let _ = self.ready.send(());
                let _ = self.release.lock().unwrap().recv();
                WakeReason::Shutdown
            }
        }
        struct OwnedScheduler {
            handle: Option<AutomationSchedulerHandle>,
            home: Option<tempfile::TempDir>,
            release: Option<std::sync::mpsc::SyncSender<()>>,
        }
        impl Drop for OwnedScheduler {
            fn drop(&mut self) {
                if let Some(release) = self.release.take() {
                    let _ = release.send(());
                }
                if let Some(handle) = self.handle.as_mut() {
                    // Cleanup hang guard, not a production promptness claim.
                    if handle
                        .shutdown_and_join_until(Instant::now() + Duration::from_secs(60))
                        .is_err()
                    {
                        if let Some(home) = self.home.take() {
                            let _ = home.keep();
                        }
                        eprintln!("controlled scheduler cleanup incomplete; owned home retained");
                    }
                }
            }
        }
        let home = tempfile::tempdir().unwrap();
        let path = home.path().to_owned();
        crate::daemon::ensure_private_coven_home(&path).unwrap();
        let (ready_tx, ready_rx) = sync_channel(1);
        let (release_tx, release_rx) = sync_channel(1);
        let clock = Arc::new(ControlledClock {
            system: SystemAutomationClock::default(),
            ready: ready_tx,
            release: Mutex::new(release_rx),
        });
        let handle = crate::automations::daemon_tick::start_automations_scheduler_with_clock(
            &path,
            Arc::new(crate::api::NoopSessionRuntime),
            clock,
        )
        .unwrap();
        let mut owned = OwnedScheduler {
            handle: Some(handle),
            home: Some(home),
            release: Some(release_tx),
        };
        let started = Instant::now();
        ready_rx
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|error| {
                panic!(
                    "controlled scheduler did not reach its gate after {:?}: {error}",
                    started.elapsed()
                )
            });
        let mut trace = trace();
        let release = owned.release.take().unwrap();
        let result = super::super::finish_startup_reconciliation_scheduler(
            owned.handle.take().unwrap(),
            owned.home.take().unwrap(),
            Instant::now(),
            &mut trace,
            || release.send(()).unwrap(),
        );
        assert_eq!(result.unwrap_err(), "conformance suite execution failed");
        let report = trace
            .failure_report(&Err("conformance suite execution failed"))
            .unwrap();
        assert!(
            report.contains("phase=first-join outcome=deadline-expired"),
            "{report}"
        );
        assert!(report.contains("phase=cleanup-join outcome=ok"), "{report}");
        assert!(!report.contains("phase=retain-home"), "{report}");
        assert!(!path.exists(), "joined worker home was not removed");
    }

    #[test]
    fn overflow_is_bounded_and_explicit() {
        let mut trace = trace();
        for _ in 0..MAX_RECORDS + 1 {
            trace.event(StartupWakePhase::Comparison, StartupWakeOutcome::Ok);
        }
        assert_eq!(trace.records.len(), MAX_RECORDS);
        assert!(trace
            .failure_report(&Err("conformance suite execution failed"))
            .unwrap()
            .contains("overflow=true"));
    }
}
