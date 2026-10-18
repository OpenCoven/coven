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
    ReservationSizeBegin,
    ReservationSizeReady,
    ReservationIdentityBegin,
    ReservationIdentityReady,
    ReservationActiveCheckReady,
    ReservationWalCheckReady,
    ReservationTruncateBegin,
    ReservationTruncateReady,
    ReservationLockWait,
    ReservationLockAcquired,
    ReservationLedgerReady,
    ReservationCommitBegin,
    ReservationCommitReady,
    ReservationActivationReady,
    ReservationReleaseLockWait,
    ReservationReleaseLockAcquired,
    ReservationReleaseDeleteReady,
    ReservationReleaseCommitReady,
    ReservationReleaseActiveReady,
    ReservationReady,
    DurableModeBegin,
    DurableModeReady,
    DurableDirectoryOpenBegin,
    DurableDirectoryOpenReady,
    DurableParentOpenBegin,
    DurableParentOpenReady,
    DurableTransactionBegin,
    DurableTransactionReady,
    DurableWriteBegin,
    DurableWriteReady,
    DurableCommitBegin,
    DurableCommitReady,
    DurableDirectorySyncBegin,
    DurableDirectorySyncReady,
    DurableParentSyncBegin,
    DurableParentSyncReady,
    DurableRestoreBegin,
    DurableRestoreReady,
    #[cfg(windows)]
    DirectoryNativeVolumeBegin,
    #[cfg(windows)]
    DirectoryNativeVolumeReady,
    DirectoryNativeReopenBegin,
    DirectoryNativeReopenReady,
    DirectoryNativeFlushBegin,
    DirectoryNativeFlushReady,
    #[cfg(windows)]
    DirectoryNativeCloseBegin,
    #[cfg(windows)]
    DirectoryNativeCloseReady,
    SchedulerPassLockWait,
    SchedulerPassLockAcquired,
    SchedulerAuditLockWait,
    SchedulerAuditLockAcquired,
    SchedulerReconcileReady,
    SchedulerCandidatesReady,
    SchedulerDocumentReady,
    SchedulerStoreReady,
    SchedulerValidationReady,
    SchedulerStoreCloseBegin,
    SchedulerStoreCloseReady,
    SchedulerDecisionBegin,
    SchedulerDecisionReturned,
    SchedulerCursorBegin,
    SchedulerCursorReady,
    SchedulerPassReady,
    #[cfg(feature = "threads-test-clock")]
    TickLockWait,
    #[cfg(feature = "threads-test-clock")]
    TickLockAcquired,
    #[cfg(feature = "threads-test-clock")]
    TickAuthorized,
    #[cfg(feature = "threads-test-clock")]
    TickWorkersReady,
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
            Self::ReservationSizeBegin => "reservation-size-begin",
            Self::ReservationSizeReady => "reservation-size-ready",
            Self::ReservationIdentityBegin => "reservation-identity-begin",
            Self::ReservationIdentityReady => "reservation-identity-ready",
            Self::ReservationActiveCheckReady => "reservation-active-check-ready",
            Self::ReservationWalCheckReady => "reservation-wal-check-ready",
            Self::ReservationTruncateBegin => "reservation-truncate-begin",
            Self::ReservationTruncateReady => "reservation-truncate-ready",
            Self::ReservationLockWait => "reservation-lock-wait",
            Self::ReservationLockAcquired => "reservation-lock-acquired",
            Self::ReservationLedgerReady => "reservation-ledger-ready",
            Self::ReservationCommitBegin => "reservation-commit-begin",
            Self::ReservationCommitReady => "reservation-commit-ready",
            Self::ReservationActivationReady => "reservation-activation-ready",
            Self::ReservationReleaseLockWait => "reservation-release-lock-wait",
            Self::ReservationReleaseLockAcquired => "reservation-release-lock-acquired",
            Self::ReservationReleaseDeleteReady => "reservation-release-delete-ready",
            Self::ReservationReleaseCommitReady => "reservation-release-commit-ready",
            Self::ReservationReleaseActiveReady => "reservation-release-active-ready",
            Self::ReservationReady => "reservation-ready",
            Self::DurableModeBegin => "durable-mode-begin",
            Self::DurableModeReady => "durable-mode-ready",
            Self::DurableDirectoryOpenBegin => "durable-directory-open-begin",
            Self::DurableDirectoryOpenReady => "durable-directory-open-ready",
            Self::DurableParentOpenBegin => "durable-parent-open-begin",
            Self::DurableParentOpenReady => "durable-parent-open-ready",
            Self::DurableTransactionBegin => "durable-transaction-begin",
            Self::DurableTransactionReady => "durable-transaction-ready",
            Self::DurableWriteBegin => "durable-write-begin",
            Self::DurableWriteReady => "durable-write-ready",
            Self::DurableCommitBegin => "durable-commit-begin",
            Self::DurableCommitReady => "durable-commit-ready",
            Self::DurableDirectorySyncBegin => "durable-directory-sync-begin",
            Self::DurableDirectorySyncReady => "durable-directory-sync-ready",
            Self::DurableParentSyncBegin => "durable-parent-sync-begin",
            Self::DurableParentSyncReady => "durable-parent-sync-ready",
            Self::DurableRestoreBegin => "durable-restore-begin",
            Self::DurableRestoreReady => "durable-restore-ready",
            #[cfg(windows)]
            Self::DirectoryNativeVolumeBegin => "directory-native-volume-begin",
            #[cfg(windows)]
            Self::DirectoryNativeVolumeReady => "directory-native-volume-ready",
            Self::DirectoryNativeReopenBegin => "directory-native-reopen-begin",
            Self::DirectoryNativeReopenReady => "directory-native-reopen-ready",
            Self::DirectoryNativeFlushBegin => "directory-native-flush-begin",
            Self::DirectoryNativeFlushReady => "directory-native-flush-ready",
            #[cfg(windows)]
            Self::DirectoryNativeCloseBegin => "directory-native-close-begin",
            #[cfg(windows)]
            Self::DirectoryNativeCloseReady => "directory-native-close-ready",
            Self::SchedulerPassLockWait => "scheduler-pass-lock-wait",
            Self::SchedulerPassLockAcquired => "scheduler-pass-lock-acquired",
            Self::SchedulerAuditLockWait => "scheduler-audit-lock-wait",
            Self::SchedulerAuditLockAcquired => "scheduler-audit-lock-acquired",
            Self::SchedulerReconcileReady => "scheduler-reconcile-ready",
            Self::SchedulerCandidatesReady => "scheduler-candidates-ready",
            Self::SchedulerDocumentReady => "scheduler-document-ready",
            Self::SchedulerStoreReady => "scheduler-store-ready",
            Self::SchedulerValidationReady => "scheduler-validation-ready",
            Self::SchedulerStoreCloseBegin => "scheduler-store-close-begin",
            Self::SchedulerStoreCloseReady => "scheduler-store-close-ready",
            Self::SchedulerDecisionBegin => "scheduler-decision-begin",
            Self::SchedulerDecisionReturned => "scheduler-decision-returned",
            Self::SchedulerCursorBegin => "scheduler-cursor-begin",
            Self::SchedulerCursorReady => "scheduler-cursor-ready",
            Self::SchedulerPassReady => "scheduler-pass-ready",
            Self::TickLockWait => "tick-lock-wait",
            Self::TickLockAcquired => "tick-lock-acquired",
            Self::TickAuthorized => "tick-authorized",
            Self::TickWorkersReady => "tick-workers-ready",
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
    if method == "POST"
        && ((path.starts_with("/api/v1/familiars/") && path.ends_with("/edits"))
            || path == "/api/v1/internal/threads/test-clock/tick")
    {
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
            state.append_with(
                phase,
                std::time::Instant::now,
                sample_native_clock,
                |line| {
                    crate::daemon::append_daemon_recovery_log(&home, line);
                },
            );
        }
    });
    let _ = phase;
}

#[cfg(feature = "threads-test-clock")]
struct NativeClock {
    process_id: u32,
    thread_id: u32,
    counter: Option<(i64, i64)>,
}

#[cfg(all(feature = "threads-test-clock", any(windows, test)))]
fn cached_native_frequency(
    cache: &std::sync::OnceLock<i64>,
    query: impl FnOnce() -> Option<i64>,
) -> Option<i64> {
    if let Some(value) = cache.get() {
        return Some(*value);
    }
    let value = query()?;
    if value <= 0 {
        return None;
    }
    let _ = cache.set(value);
    cache.get().copied()
}

#[cfg(all(feature = "threads-test-clock", windows))]
fn sample_native_clock() -> Option<NativeClock> {
    use windows_sys::Win32::System::{
        Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
        Threading::GetCurrentThreadId,
    };
    static FREQUENCY: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let frequency = cached_native_frequency(&FREQUENCY, || {
        let mut value = 0;
        // SAFETY: value is writable storage for the synchronous output parameter.
        (unsafe { QueryPerformanceFrequency(&mut value) } != 0).then_some(value)
    });
    let counter = frequency.and_then(|frequency| {
        let mut value = 0;
        // SAFETY: value is writable storage for the synchronous output parameter.
        (unsafe { QueryPerformanceCounter(&mut value) } != 0 && value >= 0)
            .then_some((value, frequency))
    });
    Some(NativeClock {
        process_id: std::process::id(),
        // SAFETY: this argument-free query reads the calling thread's numeric ID.
        thread_id: unsafe { GetCurrentThreadId() },
        counter,
    })
}

#[cfg(all(feature = "threads-test-clock", not(windows)))]
fn sample_native_clock() -> Option<NativeClock> {
    None
}

#[cfg(feature = "threads-test-clock")]
impl State {
    fn append_with(
        &mut self,
        phase: Phase,
        mut now: impl FnMut() -> std::time::Instant,
        sample: impl FnOnce() -> Option<NativeClock>,
        write: impl FnOnce(&str),
    ) {
        let observed = now();
        let native = sample();
        let mut line = format!(
            "threads_request_checkpoint request={} phase={} elapsed_us={} prior_observer_us={}",
            self.request,
            phase.label(),
            observed.saturating_duration_since(self.started).as_micros(),
            self.observer.as_micros(),
        );
        if let Some(native) = native {
            use std::fmt::Write as _;
            let _ = write!(
                line,
                " native_pid={} native_tid={}",
                native.process_id, native.thread_id
            );
            if let Some((counter, frequency)) = native.counter {
                let _ = write!(line, " native_qpc={counter} native_qpc_hz={frequency}");
            } else {
                line.push_str(" native_clock=unavailable");
            }
        }
        write(&line);
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
        for path in [
            "/api/v1/familiars/sage/edits",
            "/api/v1/internal/threads/test-clock/tick",
        ] {
            let trace = begin(home.path(), "POST", path);
            checkpoint(Phase::Intake);
            drop(trace);
        }
        assert!(!home.path().join("daemon-recovery.log").exists());
    }

    #[cfg(feature = "threads-test-clock")]
    fn fixture_home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        super::super::seed_fixture_for_tests(
            home.path(),
            "synthetic-diagnostic-capability",
            time::OffsetDateTime::UNIX_EPOCH,
        )
        .unwrap();
        home
    }

    #[cfg(feature = "threads-test-clock")]
    fn phases(home: &Path) -> Vec<String> {
        std::fs::read_to_string(home.join("daemon-recovery.log"))
            .unwrap()
            .lines()
            .filter_map(|line| {
                line.split_once(" phase=")
                    .map(|(_, tail)| tail.split_whitespace().next().unwrap().to_owned())
            })
            .collect()
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn request_diagnostics_trace_tick_through_response_without_private_fields() {
        let home = fixture_home();
        let trace = begin(
            home.path(),
            "POST",
            "/api/v1/internal/threads/test-clock/tick",
        );
        checkpoint(Phase::HandlerReturned);
        checkpoint(Phase::ResponseBegin);
        checkpoint(Phase::ResponseReady);
        drop(trace);
        assert_eq!(
            phases(home.path()),
            [
                "request-begin",
                "handler-returned",
                "response-begin",
                "response-ready",
                "request-end",
            ]
        );
        let log = std::fs::read_to_string(home.path().join("daemon-recovery.log")).unwrap();
        assert!(!log.contains("synthetic-diagnostic-capability"));
        assert!(!log.contains(&*home.path().to_string_lossy()));
        assert!(!log.contains("/api/v1"));
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn request_diagnostics_ignore_unselected_routes_in_active_fixture() {
        let home = fixture_home();
        for (method, path) in [
            ("GET", "/api/v1/internal/threads/test-clock/tick"),
            ("POST", "/api/v1/internal/threads/test-clock"),
            ("POST", "/api/v1/internal/threads/test-clock/tick/extra"),
            ("POST", "/api/v1/automations/tick"),
        ] {
            let trace = begin(home.path(), method, path);
            checkpoint(Phase::Intake);
            drop(trace);
        }
        assert!(!home.path().join("daemon-recovery.log").exists());
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn request_diagnostics_append_failure_does_not_change_reservation_outcome() {
        let home = fixture_home();
        let path = home.path().join("coven.sqlite3");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        std::fs::create_dir(home.path().join("daemon-recovery.log")).unwrap();
        let trace = begin(home.path(), "POST", "/api/v1/familiars/sage/edits");
        let required = crate::store::ward_audit_reservation_bytes(&conn, 1, 0).unwrap();
        crate::store::WardAuditReservation::acquire(
            &conn,
            &path,
            "synthetic-append-failure",
            "diagnostic-test",
            required,
        )
        .unwrap()
        .finish()
        .unwrap();
        drop(trace);
        let reserved: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM coven_ward_audit_reservations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reserved, 0);
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn request_diagnostics_partition_real_reservation_acquisition_and_release() {
        let home = fixture_home();
        let path = home.path().join("coven.sqlite3");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let trace = begin(home.path(), "POST", "/api/v1/familiars/sage/edits");
        let required = crate::store::ward_audit_reservation_bytes(&conn, 1, 0).unwrap();
        crate::store::WardAuditReservation::acquire(
            &conn,
            &path,
            "synthetic-private-reservation",
            "synthetic-private-purpose",
            required,
        )
        .unwrap()
        .finish()
        .unwrap();
        drop(trace);
        let observed = phases(home.path());
        let expected = [
            "reservation-size-begin",
            "reservation-size-ready",
            "reservation-identity-begin",
            "reservation-identity-ready",
            "reservation-active-check-ready",
            "reservation-wal-check-ready",
            "reservation-lock-wait",
            "reservation-lock-acquired",
            "reservation-ledger-ready",
            "reservation-commit-begin",
            "reservation-commit-ready",
            "reservation-activation-ready",
            "reservation-release-lock-wait",
            "reservation-release-lock-acquired",
            "reservation-release-delete-ready",
            "reservation-release-commit-ready",
            "reservation-release-active-ready",
        ];
        let actual: Vec<_> = observed
            .iter()
            .filter(|phase| phase.starts_with("reservation-"))
            .map(String::as_str)
            .collect();
        assert_eq!(actual, expected);
        let log = std::fs::read_to_string(home.path().join("daemon-recovery.log")).unwrap();
        assert!(!log.contains("synthetic-private-"));
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn request_diagnostics_partition_empty_scheduler_pass() {
        let home = fixture_home();
        crate::store::initialize_store(&home.path().join("coven.sqlite3")).unwrap();
        let trace = begin(
            home.path(),
            "POST",
            "/api/v1/internal/threads/test-clock/tick",
        );
        assert_eq!(
            crate::api::process_due_threads_proposals(home.path()).unwrap(),
            0
        );
        drop(trace);
        let observed = phases(home.path());
        for phase in [
            "scheduler-pass-lock-wait",
            "scheduler-pass-lock-acquired",
            "scheduler-audit-lock-wait",
            "scheduler-audit-lock-acquired",
            "scheduler-reconcile-ready",
            "scheduler-candidates-ready",
            "scheduler-pass-ready",
        ] {
            assert!(
                observed.iter().any(|value| value == phase),
                "{phase}: {observed:?}"
            );
        }
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
                || None,
                |line| lines.push(line.to_owned()),
            );
        }
        assert_eq!(lines, [
            "threads_request_checkpoint request=7 phase=stage-begin elapsed_us=1000 prior_observer_us=0",
            "threads_request_checkpoint request=7 phase=stage-ready elapsed_us=54000 prior_observer_us=50000",
        ]);
        assert_eq!(state.observer, Duration::from_millis(56));
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn request_diagnostics_native_samples_remain_inside_observer_accounting() {
        use std::cell::Cell;
        use std::time::{Duration, Instant};

        for (counter, suffix) in [
            (
                Some((17, 19)),
                " native_pid=11 native_tid=13 native_qpc=17 native_qpc_hz=19",
            ),
            (
                None,
                " native_pid=11 native_tid=13 native_clock=unavailable",
            ),
        ] {
            let start = Instant::now();
            let clock_ms = Cell::new(1);
            let sampled = Cell::new(false);
            let mut state = State {
                home: "synthetic-private-path".into(),
                request: 7,
                started: start,
                observer: Duration::ZERO,
            };
            state.append_with(
                Phase::StageBegin,
                || start + Duration::from_millis(clock_ms.get()),
                || {
                    assert_eq!(clock_ms.get(), 1);
                    sampled.set(true);
                    clock_ms.set(31);
                    Some(NativeClock { process_id: 11, thread_id: 13, counter })
                },
                |line| {
                    assert!(sampled.get());
                    assert_eq!(clock_ms.get(), 31);
                    assert_eq!(line, format!("threads_request_checkpoint request=7 phase=stage-begin elapsed_us=1000 prior_observer_us=0{suffix}"));
                    assert!(!line.contains("synthetic-private"));
                    clock_ms.set(51);
                },
            );
            assert_eq!(state.observer, Duration::from_millis(50));
        }
    }

    #[cfg(all(feature = "threads-test-clock", windows))]
    #[test]
    fn request_diagnostics_windows_checkpoints_match_native_clock_and_thread() {
        use windows_sys::Win32::System::{
            Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
            Threading::GetCurrentThreadId,
        };
        let home = fixture_home();
        let mut frequency = 0;
        // SAFETY: frequency is writable storage for the synchronous output.
        assert_ne!(unsafe { QueryPerformanceFrequency(&mut frequency) }, 0);
        assert!(frequency > 0);
        // SAFETY: this argument-free query reads the current thread's numeric ID.
        let thread_id = unsafe { GetCurrentThreadId() };
        let mut before = 0;
        // SAFETY: before is writable storage for the synchronous output.
        assert_ne!(unsafe { QueryPerformanceCounter(&mut before) }, 0);
        let trace = begin(home.path(), "POST", "/api/v1/familiars/sage/edits");
        checkpoint(Phase::Intake);
        drop(trace);
        let mut after = 0;
        // SAFETY: after is writable storage for the synchronous output.
        assert_ne!(unsafe { QueryPerformanceCounter(&mut after) }, 0);
        let log = std::fs::read_to_string(home.path().join("daemon-recovery.log")).unwrap();
        let records: Vec<_> = log
            .lines()
            .filter(|line| line.contains("threads_request_checkpoint "))
            .collect();
        assert_eq!(records.len(), 3, "activated checkpoints missing: {log}");
        for record in records {
            let number = |prefix: &str| -> i64 {
                record
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix(prefix))
                    .unwrap_or_else(|| panic!("missing {prefix}: {record}"))
                    .parse()
                    .unwrap()
            };
            assert_eq!(number("native_pid="), i64::from(std::process::id()));
            assert_eq!(number("native_tid="), i64::from(thread_id));
            assert_eq!(number("native_qpc_hz="), frequency);
            let counter = number("native_qpc=");
            assert!(
                counter >= before && counter <= after,
                "native sample {counter} outside observed counter interval {before}..={after}"
            );
        }
    }

    #[cfg(feature = "threads-test-clock")]
    #[test]
    fn request_diagnostics_frequency_cache_retries_failures_and_caches_only_valid_values() {
        let cache = std::sync::OnceLock::new();
        assert_eq!(cached_native_frequency(&cache, || None), None);
        assert_eq!(cached_native_frequency(&cache, || Some(0)), None);
        assert_eq!(cached_native_frequency(&cache, || Some(-1)), None);
        assert!(cache.get().is_none());
        assert_eq!(cached_native_frequency(&cache, || Some(19)), Some(19));
        assert_eq!(
            cached_native_frequency(&cache, || panic!("valid frequency was queried twice")),
            Some(19)
        );
    }
}
