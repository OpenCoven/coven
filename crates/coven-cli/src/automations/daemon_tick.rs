//! Daemon-side automations tick (coven#816).
//!
//! The daemon reconciles terminal session evidence before planning, recovering,
//! claiming, and dispatching occurrences on a 60-second cadence.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

const SCHEDULER_INTERVAL: Duration = Duration::from_secs(60);
#[cfg(test)]
const SCHEDULER_SHUTDOWN_JOIN_BUDGET: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct MonotonicInstant(Duration);

impl MonotonicInstant {
    #[cfg(test)]
    const ZERO: Self = Self(Duration::ZERO);

    fn saturating_add(self, duration: Duration) -> Self {
        Self(self.0.saturating_add(duration))
    }

    fn saturating_duration_since(self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WakeReason {
    Deadline,
    Signaled,
    Shutdown,
}

trait AutomationClock: Send + Sync {
    fn now_utc(&self) -> chrono::DateTime<chrono::Utc>;
    fn monotonic_now(&self) -> MonotonicInstant;
    fn sleep_until_or_wake(
        &self,
        deadline: MonotonicInstant,
        wake: &AutomationWakeSignal,
        observed_generation: u64,
    ) -> WakeReason;
}

struct SystemAutomationClock {
    monotonic_origin: Instant,
}

impl Default for SystemAutomationClock {
    fn default() -> Self {
        Self {
            monotonic_origin: Instant::now(),
        }
    }
}

impl AutomationClock for SystemAutomationClock {
    fn now_utc(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }

    fn monotonic_now(&self) -> MonotonicInstant {
        MonotonicInstant(self.monotonic_origin.elapsed())
    }

    fn sleep_until_or_wake(
        &self,
        deadline: MonotonicInstant,
        wake: &AutomationWakeSignal,
        observed_generation: u64,
    ) -> WakeReason {
        let mut state = wake
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if state.shutdown {
                return WakeReason::Shutdown;
            }
            if state.generation != observed_generation {
                return WakeReason::Signaled;
            }
            let remaining = deadline.saturating_duration_since(self.monotonic_now());
            if remaining.is_zero() {
                return WakeReason::Deadline;
            }
            let waited = wake
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = waited.0;
        }
    }
}

#[derive(Debug, Default)]
struct AutomationWakeState {
    generation: u64,
    shutdown: bool,
}

#[derive(Debug, Default)]
struct AutomationWakeSignal {
    state: Mutex<AutomationWakeState>,
    changed: Condvar,
}

impl AutomationWakeSignal {
    fn generation(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .generation
    }

    fn wake(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.generation = state.generation.wrapping_add(1);
        self.changed.notify_all();
    }

    fn is_shutdown(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .shutdown
    }

    fn shutdown(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.shutdown = true;
        self.changed.notify_all();
    }
}

static SCHEDULER_WAKES: OnceLock<Mutex<HashMap<PathBuf, Weak<AutomationWakeSignal>>>> =
    OnceLock::new();

fn scheduler_wakes() -> &'static Mutex<HashMap<PathBuf, Weak<AutomationWakeSignal>>> {
    SCHEDULER_WAKES.get_or_init(|| Mutex::new(HashMap::new()))
}

struct SchedulerWakeRegistration {
    home: PathBuf,
    wake: Weak<AutomationWakeSignal>,
}

impl Drop for SchedulerWakeRegistration {
    fn drop(&mut self) {
        let mut registrations = scheduler_wakes()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if registrations
            .get(&self.home)
            .is_some_and(|registered| Weak::ptr_eq(registered, &self.wake))
        {
            registrations.remove(&self.home);
        }
    }
}

fn register_scheduler_wake(
    coven_home: &Path,
    wake: Weak<AutomationWakeSignal>,
) -> Result<SchedulerWakeRegistration> {
    let home = coven_home.to_path_buf();
    let mut registrations = scheduler_wakes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if registrations.get(&home).and_then(Weak::upgrade).is_some() {
        anyhow::bail!(
            "automations scheduler is already registered for {}",
            home.display()
        );
    }
    registrations.insert(home.clone(), wake.clone());
    Ok(SchedulerWakeRegistration { home, wake })
}

pub(crate) fn wake_automations_scheduler(coven_home: &Path) -> bool {
    let wake = scheduler_wakes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(coven_home)
        .and_then(Weak::upgrade);
    if let Some(wake) = wake {
        wake.wake();
        true
    } else {
        false
    }
}

/// One automations pass: open the store, run the full tick (plan, recover,
/// claim), then dispatch every claimed occurrence through the shared
/// session-launch runtime. Failures land in the daemon recovery log via the
/// caller.
#[cfg(test)]
fn process_automations_tick(
    coven_home: &Path,
    runtime: &dyn crate::api::SessionRuntime,
) -> Result<super::occurrences::TickReport> {
    process_automations_pass(
        coven_home,
        runtime,
        &SystemAutomationClock::default(),
        &AutomationWakeSignal::default(),
        false,
    )
}

fn process_automations_pass(
    coven_home: &Path,
    runtime: &dyn crate::api::SessionRuntime,
    clock: &dyn AutomationClock,
    wake: &AutomationWakeSignal,
    startup: bool,
) -> Result<super::occurrences::TickReport> {
    let store_path = crate::api::store_path(coven_home);
    let conn = crate::store::open_store(&store_path)?;
    let now = clock.now_utc();
    reconcile_automation_runs(coven_home, &conn, runtime, now, startup)?;
    let report = super::occurrences::tick(&conn, now)?;
    let _dispatch = super::runner::dispatch_claimed_occurrences_with_clock_and_cancel(
        &conn,
        runtime,
        now,
        || clock.now_utc(),
        || wake.is_shutdown(),
    )
    .map_err(anyhow::Error::msg)?;
    Ok(report)
}

fn reconcile_automation_runs(
    coven_home: &Path,
    conn: &rusqlite::Connection,
    runtime: &dyn crate::api::SessionRuntime,
    now: chrono::DateTime<chrono::Utc>,
    startup: bool,
) -> Result<()> {
    super::runner::recover_restart_containment(coven_home, conn, now, startup)
        .map_err(anyhow::Error::msg)?;
    for failure in
        super::runner::recover_abandoned_launches(conn, runtime, now).map_err(anyhow::Error::msg)?
    {
        crate::daemon::append_daemon_recovery_log(coven_home, &failure);
    }
    for failure in
        super::runner::enforce_run_timeouts(conn, runtime, now).map_err(anyhow::Error::msg)?
    {
        crate::daemon::append_daemon_recovery_log(coven_home, &failure);
    }
    super::runner::settle_finished_runs(conn, now).map_err(anyhow::Error::msg)?;
    super::runner::cleanup_terminal_containment_receipts(coven_home, conn)
        .map_err(anyhow::Error::msg)?;
    Ok(())
}

pub struct AutomationSchedulerHandle {
    wake: Arc<AutomationWakeSignal>,
    thread: Option<JoinHandle<()>>,
}

impl AutomationSchedulerHandle {
    pub fn request_shutdown(&self) {
        self.wake.shutdown();
    }

    pub fn finish_shutdown(mut self, deadline: Instant) -> Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if thread.is_finished() {
            thread.join().map_err(|_| {
                anyhow::anyhow!("automations scheduler thread panicked during shutdown")
            })
        } else {
            eprintln!(
                "coven daemon: automations scheduler did not stop within the shared shutdown window; detaching for process exit"
            );
            Ok(())
        }
    }

    #[cfg(test)]
    fn shutdown(self) -> Result<()> {
        let deadline = Instant::now() + SCHEDULER_SHUTDOWN_JOIN_BUDGET;
        self.request_shutdown();
        self.finish_shutdown(deadline)
    }
}

impl Drop for AutomationSchedulerHandle {
    fn drop(&mut self) {
        self.wake.shutdown();
    }
}

#[cfg(test)]
fn run_automations_scheduler(
    coven_home: &Path,
    runtime: &dyn crate::api::SessionRuntime,
    clock: &dyn AutomationClock,
    wake: &AutomationWakeSignal,
) -> Result<()> {
    let observed_generation = wake.generation();
    process_automations_pass(coven_home, runtime, clock, wake, true)?;
    run_automations_scheduler_after_startup(coven_home, runtime, clock, wake, observed_generation);
    Ok(())
}

fn run_automations_scheduler_after_startup(
    coven_home: &Path,
    runtime: &dyn crate::api::SessionRuntime,
    clock: &dyn AutomationClock,
    wake: &AutomationWakeSignal,
    mut observed_generation: u64,
) {
    loop {
        let deadline = clock.monotonic_now().saturating_add(SCHEDULER_INTERVAL);
        match clock.sleep_until_or_wake(deadline, wake, observed_generation) {
            WakeReason::Shutdown => return,
            WakeReason::Deadline | WakeReason::Signaled => {
                observed_generation = wake.generation();
            }
        }
        if let Err(error) = process_automations_pass(coven_home, runtime, clock, wake, false) {
            crate::daemon::append_daemon_recovery_log(
                coven_home,
                &format!("automations tick failed: {error:#}"),
            );
        }
    }
}

/// Starts the automations scheduler. The worker immediately performs startup
/// reconciliation and due-work dispatch without delaying transport readiness.
/// The returned handle wakes and joins the worker on daemon shutdown.
pub fn start_automations_scheduler(
    coven_home: &Path,
    runtime: Arc<dyn crate::api::SessionRuntime + Send + Sync>,
) -> Result<AutomationSchedulerHandle> {
    let wake = Arc::new(AutomationWakeSignal::default());
    let registration = register_scheduler_wake(coven_home, Arc::downgrade(&wake))?;

    let store_path = crate::api::store_path(coven_home);
    let conn = crate::store::open_store(&store_path)?;
    let recovery_now = chrono::Utc::now();
    super::runner::recover_no_process_preownership_launches(coven_home, &conn, recovery_now)
        .map_err(anyhow::Error::msg)?;
    super::runner::restore_unlaunched_daemon_claims_for_retry(&conn, recovery_now)
        .map_err(anyhow::Error::msg)?;
    drop(conn);

    let clock = Arc::new(SystemAutomationClock::default());
    let observed_generation = wake.generation();

    let home = coven_home.to_path_buf();
    let thread_wake = Arc::clone(&wake);
    let thread = std::thread::Builder::new()
        .name("coven-automations-scheduler".into())
        .spawn(move || {
            let _registration = registration;
            if let Err(error) = process_automations_pass(
                &home,
                runtime.as_ref(),
                clock.as_ref(),
                thread_wake.as_ref(),
                true,
            ) {
                crate::daemon::append_daemon_recovery_log(
                    &home,
                    &format!("automations startup tick failed: {error:#}"),
                );
            }
            if thread_wake.is_shutdown() {
                return;
            }
            run_automations_scheduler_after_startup(
                &home,
                runtime.as_ref(),
                clock.as_ref(),
                thread_wake.as_ref(),
                observed_generation,
            );
        })
        .context("failed to spawn automations scheduler")?;
    Ok(AutomationSchedulerHandle {
        wake,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::store::insert_definition;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
    use std::sync::{Arc, Mutex};

    fn definition(id: &str) -> RoutineDefinition {
        RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": id,
            "name": id,
            "status": "ACTIVE",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "cwd": "/work/project",
            "prompt": "Do the thing."
        }))
        .unwrap()
    }

    #[test]
    fn tick_plans_and_claims_against_the_daemon_store() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        insert_definition(&conn, &definition("daily")).unwrap();
        // Backdate creation so the 09:00 slot is due at any tick hour.
        let old_created = (chrono::Utc::now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE automation_definitions SET created_at = ?1 WHERE id = 'daily'",
            rusqlite::params![old_created],
        )
        .unwrap();
        drop(conn);

        let report = process_automations_tick(home, &crate::api::NoopSessionRuntime).unwrap();
        assert_eq!(report.planned.len(), 1);
        assert_eq!(report.claimed.len(), 1);

        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // A launch acknowledgement proves only that the runtime accepted the
        // session. Terminal settlement waits for completion evidence.
        assert_eq!(state, "running");
        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "running");
        assert!(runs[0].session_id.is_some());
        assert_eq!(runs[0].finished_at, None);
    }

    #[test]
    fn tick_dispatches_a_preexisting_daemon_claim_without_new_claims() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let routine = definition("preclaimed");
        insert_definition(&conn, &routine).unwrap();
        let now = chrono::Utc::now();
        assert!(super::super::occurrences::insert_claimed_occurrence(
            &conn,
            "preexisting-daemon-claim",
            &routine.id,
            "daemon",
            60,
            now,
        )
        .unwrap());
        drop(conn);

        let report = process_automations_tick(home, &crate::api::NoopSessionRuntime).unwrap();
        assert!(report.claimed.is_empty());

        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'preexisting-daemon-claim'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
        assert_eq!(
            super::super::runs::list_runs(&conn, &routine.id, 10)
                .unwrap()
                .len(),
            1
        );
    }

    struct ScriptedClock {
        state: Mutex<ScriptedClockState>,
    }

    struct ScriptedClockState {
        now: chrono::DateTime<Utc>,
        monotonic: MonotonicInstant,
        waits: VecDeque<(chrono::Duration, Duration, WakeReason)>,
    }

    impl ScriptedClock {
        fn new(
            now: chrono::DateTime<Utc>,
            waits: impl IntoIterator<Item = (chrono::Duration, Duration, WakeReason)>,
        ) -> Self {
            Self {
                state: Mutex::new(ScriptedClockState {
                    now,
                    monotonic: MonotonicInstant::ZERO,
                    waits: waits.into_iter().collect(),
                }),
            }
        }
    }

    impl AutomationClock for ScriptedClock {
        fn now_utc(&self) -> chrono::DateTime<Utc> {
            self.state.lock().unwrap().now
        }

        fn monotonic_now(&self) -> MonotonicInstant {
            self.state.lock().unwrap().monotonic
        }

        fn sleep_until_or_wake(
            &self,
            _deadline: MonotonicInstant,
            _wake: &AutomationWakeSignal,
            _observed_generation: u64,
        ) -> WakeReason {
            let mut state = self.state.lock().unwrap();
            let (wall_advance, monotonic_advance, reason) =
                state.waits.pop_front().expect("scripted wait");
            state.now += wall_advance;
            state.monotonic = state.monotonic.saturating_add(monotonic_advance);
            reason
        }
    }

    struct FixedSystemClock {
        now: chrono::DateTime<Utc>,
        system: SystemAutomationClock,
    }

    impl AutomationClock for FixedSystemClock {
        fn now_utc(&self) -> chrono::DateTime<Utc> {
            self.now
        }

        fn monotonic_now(&self) -> MonotonicInstant {
            self.system.monotonic_now()
        }

        fn sleep_until_or_wake(
            &self,
            deadline: MonotonicInstant,
            wake: &AutomationWakeSignal,
            observed_generation: u64,
        ) -> WakeReason {
            self.system
                .sleep_until_or_wake(deadline, wake, observed_generation)
        }
    }

    struct BlockingFirstLaunchRuntime {
        launches: AtomicUsize,
        first_started: SyncSender<()>,
        first_release: Mutex<Receiver<()>>,
        second_started: SyncSender<()>,
    }

    impl crate::api::SessionRuntime for BlockingFirstLaunchRuntime {
        fn launch_session(&self, _launch: &crate::api::SessionLaunch) -> Result<()> {
            anyhow::bail!("blocking runtime requires the contained adopted launch path")
        }

        fn launch_contained_adopted_session(
            &self,
            _launch: &crate::api::SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            ownership_established: &mut dyn FnMut() -> Result<()>,
        ) -> Result<()> {
            ownership_established()?;
            match self.launches.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    self.first_started
                        .send(())
                        .context("failed to report first scheduler launch")?;
                    self.first_release
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .recv()
                        .context("failed to release first scheduler launch")?;
                }
                1 => {
                    self.second_started
                        .send(())
                        .context("failed to report second scheduler launch")?;
                }
                _ => {}
            }
            Ok(())
        }

        fn send_input(&self, _session_id: &str, _payload: &serde_json::Value) -> Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> Result<()> {
            Ok(())
        }
    }

    struct BlockingRejectedLaunchRuntime {
        started: SyncSender<()>,
        release: Mutex<Receiver<()>>,
    }

    impl crate::api::SessionRuntime for BlockingRejectedLaunchRuntime {
        fn launch_session(&self, _launch: &crate::api::SessionLaunch) -> Result<()> {
            anyhow::bail!("blocking runtime requires the contained adopted launch path")
        }

        fn launch_contained_adopted_session(
            &self,
            _launch: &crate::api::SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            _ownership_established: &mut dyn FnMut() -> Result<()>,
        ) -> Result<()> {
            self.started
                .send(())
                .context("failed to report blocked scheduler launch")?;
            self.release
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .recv()
                .context("failed to release blocked scheduler launch")?;
            Err(anyhow::Error::new(
                crate::api::RuntimeLaunchAdmissionClosedError,
            ))
        }

        fn send_input(&self, _session_id: &str, _payload: &serde_json::Value) -> Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> Result<()> {
            Ok(())
        }
    }

    fn set_created_at(conn: &rusqlite::Connection, id: &str, created_at: &str) {
        conn.execute(
            "UPDATE automation_definitions SET created_at = ?2 WHERE id = ?1",
            rusqlite::params![id, created_at],
        )
        .unwrap();
    }

    #[test]
    fn scheduler_runs_a_full_due_pass_before_its_first_wait() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        insert_definition(&conn, &definition("startup-due")).unwrap();
        set_created_at(&conn, "startup-due", "2026-08-29T08:00:00.000Z");
        drop(conn);

        let clock = ScriptedClock::new(
            Utc.with_ymd_and_hms(2026, 8, 30, 10, 0, 0).unwrap(),
            [(
                chrono::Duration::zero(),
                Duration::ZERO,
                WakeReason::Shutdown,
            )],
        );
        let wake = Arc::new(AutomationWakeSignal::default());

        run_automations_scheduler(home, &crate::api::NoopSessionRuntime, &clock, wake.as_ref())
            .unwrap();

        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'startup-due'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
    }

    #[test]
    fn scheduler_wake_replans_without_waiting_for_the_periodic_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        insert_definition(&conn, &definition("wake-due")).unwrap();
        set_created_at(&conn, "wake-due", "2026-08-30T08:45:00.000Z");
        drop(conn);

        let clock = ScriptedClock::new(
            Utc.with_ymd_and_hms(2026, 8, 30, 8, 30, 0).unwrap(),
            [
                (
                    chrono::Duration::minutes(90),
                    Duration::from_secs(1),
                    WakeReason::Signaled,
                ),
                (
                    chrono::Duration::zero(),
                    Duration::ZERO,
                    WakeReason::Shutdown,
                ),
            ],
        );
        let wake = Arc::new(AutomationWakeSignal::default());

        run_automations_scheduler(home, &crate::api::NoopSessionRuntime, &clock, wake.as_ref())
            .unwrap();

        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let (state, failure_reason): (String, Option<String>) = conn
            .query_row(
                "SELECT state, failure_reason
                 FROM automation_occurrences
                 WHERE automation_id = 'wake-due'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "running", "{failure_reason:?}");
    }

    #[test]
    fn mutation_racing_an_active_pass_is_not_lost_before_the_next_wait() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_path_buf();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        insert_definition(&conn, &definition("first-due")).unwrap();
        set_created_at(&conn, "first-due", "2026-08-29T08:00:00.000Z");
        drop(conn);

        let (first_started_tx, first_started_rx) = sync_channel(0);
        let (first_release_tx, first_release_rx) = sync_channel(0);
        let (second_started_tx, second_started_rx) = sync_channel(0);
        let runtime = Arc::new(BlockingFirstLaunchRuntime {
            launches: AtomicUsize::new(0),
            first_started: first_started_tx,
            first_release: Mutex::new(first_release_rx),
            second_started: second_started_tx,
        });
        let clock = Arc::new(FixedSystemClock {
            now: Utc.with_ymd_and_hms(2026, 8, 30, 10, 0, 0).unwrap(),
            system: SystemAutomationClock::default(),
        });
        let wake = Arc::new(AutomationWakeSignal::default());
        let scheduler_home = home.clone();
        let scheduler_runtime = Arc::clone(&runtime);
        let scheduler_clock = Arc::clone(&clock);
        let scheduler_wake = Arc::clone(&wake);
        let scheduler = std::thread::spawn(move || {
            run_automations_scheduler(
                &scheduler_home,
                scheduler_runtime.as_ref(),
                scheduler_clock.as_ref(),
                scheduler_wake.as_ref(),
            )
        });

        first_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("startup dispatch should reach the blocking runtime");
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        insert_definition(&conn, &definition("second-due")).unwrap();
        set_created_at(&conn, "second-due", "2026-08-29T08:00:00.000Z");
        drop(conn);
        wake.wake();
        first_release_tx.send(()).unwrap();

        // This is a hang guard far below the 60-second periodic deadline, not
        // a promptness assertion. The real wake must start the second pass.
        let second_started = second_started_rx.recv_timeout(Duration::from_secs(5));
        wake.shutdown();
        scheduler.join().unwrap().unwrap();
        second_started.expect("the pass-racing wake should dispatch the second definition");
        assert_eq!(runtime.launches.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn scheduler_start_does_not_block_daemon_readiness_on_due_dispatch() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        insert_definition(&conn, &definition("startup-blocked")).unwrap();
        set_created_at(&conn, "startup-blocked", "2020-01-01T08:00:00.000Z");
        drop(conn);

        let (started_tx, started_rx) = sync_channel(0);
        let (release_tx, release_rx) = sync_channel(0);
        let runtime = Arc::new(BlockingRejectedLaunchRuntime {
            started: started_tx,
            release: Mutex::new(release_rx),
        });

        let scheduler_home = home.to_path_buf();
        let (handle_tx, handle_rx) = sync_channel(1);
        let starter = std::thread::spawn(move || {
            let _ = handle_tx.send(start_automations_scheduler(&scheduler_home, runtime));
        });
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("startup pass should begin");

        let returned_before_dispatch = handle_rx.try_recv();
        let returned_promptly = returned_before_dispatch.is_ok();
        release_tx.send(()).unwrap();
        let handle = match returned_before_dispatch {
            Ok(result) => result.unwrap(),
            Err(_) => handle_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("scheduler start should eventually return")
                .unwrap(),
        };
        starter.join().unwrap();
        handle.request_shutdown();
        handle.shutdown().unwrap();
        assert!(
            returned_promptly,
            "scheduler startup must not wait for due runtime dispatch"
        );
    }

    #[test]
    fn shutdown_racing_preownership_launch_restores_retryable_occurrence() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_path_buf();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        insert_definition(&conn, &definition("shutdown-retry")).unwrap();
        set_created_at(&conn, "shutdown-retry", "2020-01-01T08:00:00.000Z");
        drop(conn);

        let (started_tx, started_rx) = sync_channel(0);
        let (release_tx, release_rx) = sync_channel(0);
        let runtime = Arc::new(BlockingRejectedLaunchRuntime {
            started: started_tx,
            release: Mutex::new(release_rx),
        });
        let clock = Arc::new(FixedSystemClock {
            now: Utc.with_ymd_and_hms(2026, 8, 30, 10, 0, 0).unwrap(),
            system: SystemAutomationClock::default(),
        });
        let wake = Arc::new(AutomationWakeSignal::default());
        let scheduler_home = home.clone();
        let scheduler_clock = Arc::clone(&clock);
        let scheduler_wake = Arc::clone(&wake);
        let scheduler = std::thread::spawn(move || {
            run_automations_scheduler(
                &scheduler_home,
                runtime.as_ref(),
                scheduler_clock.as_ref(),
                scheduler_wake.as_ref(),
            )
        });
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("startup dispatch should reach the blocked runtime");

        wake.shutdown();
        release_tx.send(()).unwrap();
        scheduler.join().unwrap().unwrap();

        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let (state, lease_owner, failure_reason): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT state, lease_owner, failure_reason
                 FROM automation_occurrences
                 WHERE automation_id = 'shutdown-retry'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, "planned");
        assert_eq!(lease_owner, None);
        assert_eq!(failure_reason, None);
        let run_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM automation_runs", [], |row| row.get(0))
            .unwrap();
        let session_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(run_count, 0);
        assert_eq!(session_count, 0);
    }

    #[test]
    fn successful_definition_mutation_wakes_only_its_registered_home() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        for home in [first.path(), second.path()] {
            crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        }
        let first_wake = Arc::new(AutomationWakeSignal::default());
        let second_wake = Arc::new(AutomationWakeSignal::default());
        let _first_registration =
            register_scheduler_wake(first.path(), Arc::downgrade(&first_wake)).unwrap();
        let _second_registration =
            register_scheduler_wake(second.path(), Arc::downgrade(&second_wake)).unwrap();

        let invalid_body = json!({
            "action": "coven.automations.definition.create.v1",
            "adoptionKey": "adopt:create:wake-test:invalid",
            "definition": []
        })
        .to_string();
        let invalid_response = crate::api::handle_request_with_body(
            "POST",
            "/api/v1/actions",
            first.path(),
            None,
            Some(&invalid_body),
        )
        .unwrap();
        assert_eq!(invalid_response.status, 400);
        assert_eq!(first_wake.generation(), 0);

        let body = json!({
            "action": "coven.automations.definition.create.v1",
            "adoptionKey": "adopt:create:wake-test:0001",
            "definition": {
                "schemaVersion": 1,
                "id": "wake-test",
                "name": "Wake test",
                "status": "ACTIVE",
                "rrule": "FREQ=DAILY;BYHOUR=9",
                "timezone": "utc",
                "misfire": "latest",
                "overlap": "forbid",
                "timeoutMinutes": 30,
                "runtime": "coven-code",
                "cwd": "/work/project",
                "prompt": "Do the thing."
            }
        })
        .to_string();

        let response = crate::api::handle_request_with_body(
            "POST",
            "/api/v1/actions",
            first.path(),
            None,
            Some(&body),
        )
        .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(first_wake.generation(), 1);
        assert_eq!(second_wake.generation(), 0);
    }

    #[test]
    fn wake_and_shutdown_are_observed_without_a_wall_clock_sleep() {
        let clock = SystemAutomationClock::default();
        let wake = AutomationWakeSignal::default();
        let deadline = clock
            .monotonic_now()
            .saturating_add(Duration::from_secs(60));

        wake.wake();
        assert_eq!(
            clock.sleep_until_or_wake(deadline, &wake, 0),
            WakeReason::Signaled
        );

        let observed_generation = wake.generation();
        wake.shutdown();
        assert_eq!(
            clock.sleep_until_or_wake(deadline, &wake, observed_generation),
            WakeReason::Shutdown
        );
    }

    #[test]
    fn scheduler_handle_wakes_joins_and_unregisters_its_home() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let handle =
            start_automations_scheduler(home, Arc::new(crate::api::NoopSessionRuntime)).unwrap();

        assert!(wake_automations_scheduler(home));
        handle.shutdown().unwrap();
        assert!(!wake_automations_scheduler(home));
    }
}
