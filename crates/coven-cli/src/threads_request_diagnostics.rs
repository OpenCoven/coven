use std::path::Path;

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    #[cfg(feature = "threads-test-clock")]
    RequestBegin,
    BodyRead,
    Intake,
    LockWait,
    LockAcquired,
    StoreOpen,
    StoreReady,
    ReservationBegin,
    ReservationReady,
    GateReady,
    IdentityBegin,
    IdentityReady,
    ProbesBegin,
    ProbesReady,
    SubmissionBindingBegin,
    SubmissionBindingReady,
    StageBegin,
    StageReady,
    ReceiptBegin,
    ReceiptReady,
    FinalizeBegin,
    FinalizeReady,
    HandlerReturned,
    ResponseBegin,
    ResponseReady,
    #[cfg(feature = "threads-test-clock")]
    RequestEnd,
}

#[cfg(feature = "threads-test-clock")]
impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::RequestBegin => "request-begin",
            Self::BodyRead => "body-read",
            Self::Intake => "intake",
            Self::LockWait => "lock-wait",
            Self::LockAcquired => "lock-acquired",
            Self::StoreOpen => "store-open",
            Self::StoreReady => "store-ready",
            Self::ReservationBegin => "reservation-begin",
            Self::ReservationReady => "reservation-ready",
            Self::GateReady => "gate-ready",
            Self::IdentityBegin => "identity-begin",
            Self::IdentityReady => "identity-ready",
            Self::ProbesBegin => "probes-begin",
            Self::ProbesReady => "probes-ready",
            Self::SubmissionBindingBegin => "submission-binding-begin",
            Self::SubmissionBindingReady => "submission-binding-ready",
            Self::StageBegin => "stage-begin",
            Self::StageReady => "stage-ready",
            Self::ReceiptBegin => "receipt-begin",
            Self::ReceiptReady => "receipt-ready",
            Self::FinalizeBegin => "finalize-begin",
            Self::FinalizeReady => "finalize-ready",
            Self::HandlerReturned => "handler-returned",
            Self::ResponseBegin => "response-begin",
            Self::ResponseReady => "response-ready",
            Self::RequestEnd => "request-end",
        }
    }
}

#[cfg(feature = "threads-test-clock")]
struct State {
    home: std::path::PathBuf,
    request: u64,
    started: std::time::Instant,
    observer: std::time::Duration,
}

#[cfg(feature = "threads-test-clock")]
thread_local! {
    static ACTIVE: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}

pub(crate) struct RequestTrace {
    #[cfg(feature = "threads-test-clock")]
    previous: Option<State>,
    #[cfg(feature = "threads-test-clock")]
    enabled: bool,
}

pub(crate) fn begin(home: &Path, method: &str, path: &str) -> RequestTrace {
    #[cfg(feature = "threads-test-clock")]
    if method == "POST" && path.starts_with("/api/v1/familiars/") && path.ends_with("/edits") {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
        let started = std::time::Instant::now();
        match super::fixture_mode_enabled(home) {
            Ok(true) => {
                let previous = ACTIVE.with(|active| {
                    active.replace(Some(State {
                        home: home.to_owned(),
                        request: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
                        started,
                        observer: started.elapsed(),
                    }))
                });
                checkpoint(Phase::RequestBegin);
                return RequestTrace {
                    previous,
                    enabled: true,
                };
            }
            Ok(false) => {}
            Err(_) => crate::daemon::append_daemon_recovery_log(
                home,
                "threads_request_diagnostics status=activation-error",
            ),
        }
    }
    let _ = (home, method, path);
    RequestTrace {
        #[cfg(feature = "threads-test-clock")]
        previous: None,
        #[cfg(feature = "threads-test-clock")]
        enabled: false,
    }
}

pub(crate) fn checkpoint(phase: Phase) {
    #[cfg(feature = "threads-test-clock")]
    ACTIVE.with(|active| {
        if let Some(state) = active.borrow_mut().as_mut() {
            let home = state.home.clone();
            state.append_with(phase, std::time::Instant::now, |line| {
                crate::daemon::append_daemon_recovery_log(&home, line);
            });
        }
    });
    let _ = phase;
}

#[cfg(feature = "threads-test-clock")]
impl State {
    fn append_with(
        &mut self,
        phase: Phase,
        mut now: impl FnMut() -> std::time::Instant,
        write: impl FnOnce(&str),
    ) {
        let observed = now();
        write(&format!(
            "threads_request_checkpoint request={} phase={} elapsed_us={} prior_observer_us={}",
            self.request,
            phase.label(),
            observed.saturating_duration_since(self.started).as_micros(),
            self.observer.as_micros(),
        ));
        // Match startup diagnostics: an emitted line does not prove its own
        // append returned. Later lines expose prior observer cost separately.
        self.observer += now().saturating_duration_since(observed);
    }
}

impl Drop for RequestTrace {
    fn drop(&mut self) {
        #[cfg(feature = "threads-test-clock")]
        if self.enabled {
            checkpoint(Phase::RequestEnd);
            ACTIVE.with(|active| active.replace(self.previous.take()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_diagnostics_require_fixture_activation() {
        let home = tempfile::tempdir().unwrap();
        let trace = begin(home.path(), "POST", "/api/v1/familiars/sage/edits");
        checkpoint(Phase::Intake);
        drop(trace);
        assert!(!home.path().join("daemon-recovery.log").exists());
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn request_diagnostics_separate_observer_cost_without_private_fields() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let mut state = State {
            home: "synthetic-private-path".into(),
            request: 7,
            started: start,
            observer: Duration::ZERO,
        };
        let mut times = [1, 51, 54, 60]
            .into_iter()
            .map(|ms| start + Duration::from_millis(ms));
        let mut lines = Vec::new();
        for phase in [Phase::StageBegin, Phase::StageReady] {
            state.append_with(
                phase,
                || times.next().unwrap(),
                |line| lines.push(line.to_owned()),
            );
        }
        assert_eq!(lines, [
            "threads_request_checkpoint request=7 phase=stage-begin elapsed_us=1000 prior_observer_us=0",
            "threads_request_checkpoint request=7 phase=stage-ready elapsed_us=54000 prior_observer_us=50000",
        ]);
        assert_eq!(state.observer, Duration::from_millis(56));
    }
}
