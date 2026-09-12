//! Native authority-journey admission, separate from the production launcher SLA.

use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use coven_client::ClientError;
#[cfg(any(windows, test))]
use serde_json::Value;

// Fixture hang guard, independent of production CLI lifecycle deadlines.
pub const LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(15);

pub fn start_operation(windows: bool) -> &'static str {
    if windows {
        "serve"
    } else {
        "start"
    }
}

#[cfg(windows)]
pub fn acquire_windows_admission() -> std::sync::MutexGuard<'static, ()> {
    static ADMISSION: std::sync::Mutex<()> = std::sync::Mutex::new(());
    ADMISSION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Retain the process handle even when status publication/readiness fails.
pub struct OwnedDaemon {
    child: Child,
    exit: ExitState,
    started: Instant,
    ready: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ExitEvidence {
    pub status: ExitStatus,
    pub termination_requested: bool,
}

impl ExitEvidence {
    pub fn origin(self) -> &'static str {
        if self.termination_requested {
            "fixture_termination_requested"
        } else {
            "observed_exit"
        }
    }
}

#[derive(Default)]
struct ExitState {
    status: Option<ExitStatus>,
    termination_requested: bool,
    teardown_started: Option<Instant>,
}

trait ChildControl {
    fn id(&self) -> u32;
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>>;
    fn kill(&mut self) -> std::io::Result<()>;
}

impl ChildControl for Child {
    fn id(&self) -> u32 {
        Child::id(self)
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }

    fn kill(&mut self) -> std::io::Result<()> {
        Child::kill(self)
    }
}

impl OwnedDaemon {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        let started = Instant::now();
        Ok(Self {
            child: command.spawn().context("spawning owned fixture daemon")?,
            exit: ExitState::default(),
            started,
            ready: false,
        })
    }

    pub fn started(&self) -> Instant {
        self.started
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.exit.status
    }

    pub fn exit_evidence(&self) -> Option<ExitEvidence> {
        self.exit.status.map(|status| ExitEvidence {
            status,
            termination_requested: self.exit.termination_requested,
        })
    }

    pub fn admit(&mut self, readiness: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        let Err(cause) = readiness(self) else {
            return Ok(());
        };
        match self.finalize_startup_failure() {
            Ok(_) => Err(cause),
            Err(error) => Err(cause.context(format!(
                "failed settling owned daemon {} after admission failure: {error:#}",
                self.id()
            ))),
        }
    }

    fn ensure_running(&mut self) -> Result<()> {
        if self.exit.status.is_none() {
            self.exit.status = self.child.try_wait()?;
        }
        if let Some(status) = self.exit.status {
            anyhow::bail!("fixture daemon exited before readiness: {status}");
        }
        Ok(())
    }

    pub fn mark_ready(&mut self) -> Result<()> {
        self.ensure_running()?;
        anyhow::ensure!(
            Instant::now() < self.started + LIFECYCLE_TIMEOUT,
            "fixture daemon readiness exceeded its original admission deadline",
        );
        self.ready = true;
        Ok(())
    }

    #[cfg(windows)]
    pub fn wait_for_health(&mut self, home: &std::path::Path) -> Result<()> {
        let pipe = coven_client::owner_only_windows_pipe_name(home)?;
        let pid = self.id();
        wait_for_readiness(
            pid,
            &pipe,
            self.started(),
            || self.ensure_running(),
            |deadline| {
                coven_client::probe_windows_daemon_health_with_identity_until(&pipe, deadline).map(
                    |probe| {
                        probe.map(|probe| HealthProbe::Windows {
                            server_pid: probe.server_pid,
                            status: probe.status,
                            body: probe.body,
                        })
                    },
                )
            },
            Instant::now,
            thread::sleep,
        )?;
        self.mark_ready()
    }

    #[cfg(unix)]
    pub fn wait_for_health(&mut self, home: &std::path::Path) -> Result<()> {
        let endpoint = std::fs::canonicalize(home)?
            .join("coven.sock")
            .display()
            .to_string();
        let pid = self.id();
        wait_for_readiness(
            pid,
            &endpoint,
            self.started(),
            || self.ensure_running(),
            |deadline| {
                // The client authenticates the peer PID, health contract, and
                // canonical profile/socket binding before returning this status.
                coven_client::probe_unix_daemon_health(
                    home,
                    deadline.saturating_duration_since(Instant::now()),
                )
                .map(|status| status.map(HealthProbe::Unix))
            },
            Instant::now,
            thread::sleep,
        )?;
        self.mark_ready()
    }

    pub fn reap(&mut self, terminate: bool) -> Result<ExitStatus> {
        reap_owned_process(
            &mut self.child,
            &mut self.exit,
            terminate,
            Instant::now,
            thread::sleep,
        )
    }

    pub fn finalize_startup_failure(&mut self) -> Result<ExitStatus> {
        finalize_startup_process(
            &mut self.child,
            &mut self.exit,
            self.started + LIFECYCLE_TIMEOUT,
            Instant::now,
            thread::sleep,
        )
    }
}

fn finalize_startup_process(
    child: &mut impl ChildControl,
    exit: &mut ExitState,
    admission_deadline: Instant,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(Duration),
) -> Result<ExitStatus> {
    // Fatal admission never retries health. Observe only the retained child,
    // using admission's remainder, before starting the persistent teardown.
    if exit.status.is_none() && exit.teardown_started.is_none() {
        exit.status = wait_for_exit_until(
            admission_deadline,
            || {
                child
                    .try_wait()
                    .context("observing owned fixture child exit")
            },
            &mut now,
            &mut pause,
        )?;
    }
    reap_owned_process(child, exit, true, now, pause)
}

fn reap_owned_process(
    child: &mut impl ChildControl,
    exit: &mut ExitState,
    terminate: bool,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(Duration),
) -> Result<ExitStatus> {
    if let Some(status) = exit.status {
        return Ok(status);
    }
    // A failure followed by Drop must not acquire another teardown budget.
    let started = *exit.teardown_started.get_or_insert_with(&mut now);
    let deadline = started + LIFECYCLE_TIMEOUT;
    loop {
        if let Some(status) = child
            .try_wait()
            .context("observing owned fixture child exit")?
        {
            exit.status = Some(status);
            let observed = now();
            anyhow::ensure!(
                observed < deadline,
                "owned fixture daemon {} exit was observed after teardown deadline: {:?}",
                child.id(),
                observed.saturating_duration_since(started),
            );
            return Ok(status);
        }
        let observed = now();
        if terminate && !exit.termination_requested {
            // Record the attempt, not a claim that it caused the exit: the
            // process may naturally exit between try_wait and kill.
            exit.termination_requested = true;
            child.kill().context("terminating owned fixture daemon")?;
            continue;
        }
        anyhow::ensure!(
            observed < deadline,
            "owned fixture daemon {} was not reaped after {:?}",
            child.id(),
            observed.saturating_duration_since(started),
        );
        pause(Duration::from_millis(25).min(deadline.saturating_duration_since(observed)));
    }
}

fn wait_for_exit_until(
    deadline: Instant,
    mut observe: impl FnMut() -> Result<Option<ExitStatus>>,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(Duration),
) -> Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = observe()? {
            return Ok(Some(status));
        }
        let remaining = deadline.saturating_duration_since(now());
        if remaining.is_zero() {
            return Ok(None);
        }
        pause(Duration::from_millis(25).min(remaining));
    }
}

impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        if let Err(error) = self.reap(true) {
            eprintln!(
                "failed terminating/reaping owned fixture daemon {}: {error:#}",
                self.id()
            );
        }
    }
}

enum HealthProbe {
    #[cfg(any(windows, test))]
    Windows {
        server_pid: u32,
        status: u16,
        body: Vec<u8>,
    },
    #[cfg(unix)]
    Unix(coven_client::LifecycleDaemonStatus),
}

impl HealthProbe {
    fn validate(self, pid: u32, endpoint: &str) -> Result<()> {
        match self {
            #[cfg(any(windows, test))]
            Self::Windows {
                server_pid,
                status,
                body,
            } => {
                let body: Value = serde_json::from_slice(&body)?;
                anyhow::ensure!(
                    status == 200 && body["ok"] == true,
                    "fixture daemon returned invalid health: HTTP {status} {body}",
                );
                anyhow::ensure!(
                    server_pid == pid
                        && body["daemon"]["pid"] == pid
                        && body["daemon"]["socket"] == endpoint,
                    "fixture health did not identify its owned process and pipe: server_pid={server_pid} {body}",
                );
            }
            #[cfg(unix)]
            Self::Unix(status) => {
                anyhow::ensure!(
                    status.pid == pid && status.socket == endpoint,
                    "authenticated fixture health did not identify its owned process {pid} and endpoint {endpoint}: {status:?}",
                );
            }
        }
        Ok(())
    }
}

fn wait_for_readiness(
    pid: u32,
    pipe: &str,
    started: Instant,
    mut check_child: impl FnMut() -> Result<()>,
    mut probe: impl FnMut(Instant) -> Result<Option<HealthProbe>, ClientError>,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(Duration),
) -> Result<()> {
    let deadline = started + LIFECYCLE_TIMEOUT;
    let mut last_pending = None;
    loop {
        check_child()?;
        let observed = now();
        anyhow::ensure!(
            observed < deadline,
            "fixture daemon did not become healthy after {:?}: {last_pending:?}",
            observed.saturating_duration_since(started),
        );
        match probe(deadline.min(observed + Duration::from_millis(250))) {
            Ok(Some(probe)) => {
                probe.validate(pid, pipe)?;
                check_child()?;
                let observed = now();
                anyhow::ensure!(
                    observed < deadline,
                    "fixture daemon readiness exceeded its deadline after {:?}",
                    observed.saturating_duration_since(started),
                );
                return Ok(());
            }
            Ok(None) => last_pending = Some("endpoint not yet available".to_owned()),
            Err(error) if is_pending_startup_error(&error, cfg!(unix)) => {
                last_pending = Some(error.to_string());
            }
            Err(error) => {
                let error = anyhow::Error::new(error);
                return Err(match check_child() {
                    Ok(()) => error,
                    Err(child_error) => error.context(format!(
                        "child observation after fatal health probe: {child_error:#}"
                    )),
                });
            }
        }
        pause(Duration::from_millis(25).min(deadline.saturating_duration_since(now())));
    }
}

fn is_pending_startup_error(error: &ClientError, unix: bool) -> bool {
    match error {
        ClientError::Io { source, .. } => {
            source.kind() == std::io::ErrorKind::TimedOut
                || (unix
                    && matches!(
                        source.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ))
        }
        ClientError::InvalidHttpResponse(message) => {
            message == coven_client::EMPTY_RESPONSE_TIMEOUT_MESSAGE
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    fn exit_code(code: i32) -> ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            ExitStatus::from_raw(code << 8)
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            ExitStatus::from_raw(u32::try_from(code).unwrap())
        }
    }

    struct ScriptedChild<'a> {
        clock: &'a Cell<Instant>,
        exits_at: Option<Instant>,
        exit_code_on_kill: Option<i32>,
        kills: usize,
    }

    impl ChildControl for ScriptedChild<'_> {
        fn id(&self) -> u32 {
            42
        }

        fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
            if self.exits_at.is_some_and(|at| self.clock.get() >= at) {
                Ok(Some(exit_code(17)))
            } else if self.kills > 0 {
                Ok(self.exit_code_on_kill.map(exit_code))
            } else {
                Ok(None)
            }
        }

        fn kill(&mut self) -> std::io::Result<()> {
            self.kills += 1;
            Ok(())
        }
    }

    #[test]
    fn startup_finalization_observes_natural_exit_without_requesting_termination() -> Result<()> {
        let started = Instant::now();
        let clock = Cell::new(started);
        let mut child = ScriptedChild {
            clock: &clock,
            exits_at: Some(started + Duration::from_millis(500)),
            exit_code_on_kill: Some(9),
            kills: 0,
        };
        let mut exit = ExitState::default();
        let status = finalize_startup_process(
            &mut child,
            &mut exit,
            started + LIFECYCLE_TIMEOUT,
            || clock.get(),
            |delay| clock.set(clock.get() + delay),
        )?;
        assert_eq!(status.code(), Some(17));
        assert_eq!(child.kills, 0);
        assert!(exit.teardown_started.is_none());
        assert_eq!(
            ExitEvidence {
                status,
                termination_requested: exit.termination_requested
            }
            .origin(),
            "observed_exit"
        );
        assert_eq!(clock.get(), started + Duration::from_millis(500));
        Ok(())
    }

    #[test]
    fn startup_finalization_labels_cleanup_termination_instead_of_natural_exit() -> Result<()> {
        // Even the natural startup exit code cannot establish the cause once
        // termination was requested between the last observation and exit.
        for code in [9, 17] {
            let started = Instant::now();
            let clock = Cell::new(started + LIFECYCLE_TIMEOUT - Duration::from_millis(10));
            let mut child = ScriptedChild {
                clock: &clock,
                exits_at: None,
                exit_code_on_kill: Some(code),
                kills: 0,
            };
            let mut exit = ExitState::default();
            let status = finalize_startup_process(
                &mut child,
                &mut exit,
                started + LIFECYCLE_TIMEOUT,
                || clock.get(),
                |delay| clock.set(clock.get() + delay),
            )?;
            assert_eq!(status.code(), Some(code));
            assert_eq!(child.kills, 1);
            assert_eq!(
                ExitEvidence {
                    status,
                    termination_requested: exit.termination_requested
                }
                .origin(),
                "fixture_termination_requested"
            );
            assert_eq!(clock.get(), started + LIFECYCLE_TIMEOUT);
            assert_eq!(exit.teardown_started, Some(clock.get()));
        }
        Ok(())
    }

    #[test]
    fn startup_finalization_and_fallback_share_one_absolute_teardown_budget() {
        let started = Instant::now();
        let clock = Cell::new(started);
        let mut child = ScriptedChild {
            clock: &clock,
            exits_at: None,
            exit_code_on_kill: None,
            kills: 0,
        };
        let mut exit = ExitState::default();
        let error = finalize_startup_process(
            &mut child,
            &mut exit,
            started + LIFECYCLE_TIMEOUT,
            || clock.get(),
            |delay| clock.set(clock.get() + delay),
        )
        .unwrap_err();
        assert!(error.to_string().contains("not reaped after 15s"));
        assert_eq!(clock.get(), started + 2 * LIFECYCLE_TIMEOUT);
        assert_eq!(exit.teardown_started, Some(started + LIFECYCLE_TIMEOUT));
        assert_eq!(child.kills, 1);
        assert!(exit.status.is_none());
        assert!(finalize_startup_process(
            &mut child,
            &mut exit,
            started + LIFECYCLE_TIMEOUT,
            || clock.get(),
            |_| panic!("fallback must not renew the budget"),
        )
        .is_err());
        assert_eq!(child.kills, 1);
        assert_eq!(clock.get(), started + 2 * LIFECYCLE_TIMEOUT);
    }

    fn healthy() -> HealthProbe {
        HealthProbe::Windows {
            server_pid: 42,
            status: 200,
            body: serde_json::to_vec(&json!({
                "ok": true, "daemon": {"pid": 42, "socket": "fixture-pipe"}
            }))
            .unwrap(),
        }
    }

    #[test]
    fn windows_authority_admission_does_not_use_the_cli_launcher() {
        assert_eq!(start_operation(true), "serve");
        assert_eq!(start_operation(false), "start");
    }

    #[test]
    fn owned_child_cleanup_does_not_require_status_or_health() -> Result<()> {
        #[cfg(windows)]
        let _admission = acquire_windows_admission();
        let home = tempfile::tempdir()?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
        command
            .args(["daemon", "serve"])
            .env("COVEN_HOME", home.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = OwnedDaemon::spawn(&mut command)?;
        assert!(!child.is_ready());
        child.started = Instant::now() - LIFECYCLE_TIMEOUT;
        assert!(
            child.mark_ready().is_err(),
            "a live owned child must not publish readiness after its admission deadline"
        );
        let status = child.reap(true)?;
        assert!(child.child.try_wait()?.is_some());
        assert_eq!(child.exit_status(), Some(status));
        assert_eq!(
            child
                .exit_evidence()
                .context("finalized exit evidence")?
                .status,
            status
        );
        assert!(child.ensure_running().is_err());
        assert_eq!(child.reap(true)?, status);
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn owned_unix_admission_authenticates_and_reaps_the_live_child() -> Result<()> {
        let home = tempfile::tempdir()?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
        command
            .args(["daemon", "serve"])
            .env("COVEN_HOME", home.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = OwnedDaemon::spawn(&mut command)?;
        child.wait_for_health(home.path())?;
        assert!(child.is_ready());
        child.reap(true)?;
        let status = child.exit_status().context("reaped child exit status")?;
        assert_eq!(child.child.try_wait()?, Some(status));
        assert!(child.ensure_running().is_err());
        assert!(child.mark_ready().is_err());
        child.reap(true)?;
        assert_eq!(child.exit_status(), Some(status));
        Ok(())
    }

    #[test]
    fn owned_child_normal_exit_status_survives_repeated_observation() -> Result<()> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
        command
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = OwnedDaemon::spawn(&mut command)?;
        child.reap(false)?;
        let status = child.exit_status().context("normal child exit status")?;
        assert_eq!(status.code(), Some(0));
        assert!(child.ensure_running().is_err());
        child.reap(true)?;
        assert_eq!(child.exit_status(), Some(status));
        Ok(())
    }

    #[test]
    fn failed_admission_retains_cause_and_later_natural_exit() -> Result<()> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
        command
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = OwnedDaemon::spawn(&mut command)?;
        let started = child.started();
        let error = child
            .admit(|child| {
                assert!(child.exit_status().is_none());
                Err(ClientError::Io {
                    operation: "failed to read Coven daemon response",
                    source: std::io::Error::from_raw_os_error(109),
                }
                .into())
            })
            .expect_err("natural exit must not turn failed admission into success");
        assert!(matches!(
            error.downcast_ref::<ClientError>(),
            Some(ClientError::Io { source, .. }) if source.raw_os_error() == Some(109)
        ));
        let exit = child.exit_evidence().context("settled natural exit")?;
        assert_eq!(exit.status.code(), Some(0));
        assert!(!exit.termination_requested);
        assert!(!child.is_ready());
        assert_eq!(child.started(), started, "admission budget was reset");
        child.reap(true)?;
        assert!(!child.exit_evidence().unwrap().termination_requested);
        Ok(())
    }

    #[test]
    fn fatal_probe_is_not_retried_while_observing_a_later_exit() -> Result<()> {
        let start = Instant::now();
        let deadline = start + LIFECYCLE_TIMEOUT;
        let clock = Cell::new(deadline - Duration::from_millis(50));
        let child_checks = Cell::new(0);
        let probes = Cell::new(0);
        let error = wait_for_readiness(
            42,
            "fixture-pipe",
            start,
            || {
                child_checks.set(child_checks.get() + 1);
                Ok(())
            },
            |_| {
                probes.set(probes.get() + 1);
                Err(ClientError::Io {
                    operation: "failed to read Coven daemon response",
                    source: std::io::Error::from_raw_os_error(109),
                })
            },
            || clock.get(),
            |_| panic!("a fatal probe must stop admission immediately"),
        )
        .unwrap_err();
        let observations = Cell::new(0);
        let status = wait_for_exit_until(
            deadline,
            || {
                observations.set(observations.get() + 1);
                Ok((observations.get() == 3).then(ExitStatus::default))
            },
            || clock.get(),
            |delay| clock.set(clock.get() + delay),
        )?;
        assert_eq!(status.and_then(|status| status.code()), Some(0));
        assert_eq!(clock.get(), deadline);
        assert_eq!(probes.get(), 1);
        assert_eq!(child_checks.get(), 2);
        assert_eq!(observations.get(), 3);
        assert!(matches!(
            error.downcast_ref::<ClientError>(),
            Some(ClientError::Io { source, .. }) if source.raw_os_error() == Some(109)
        ));
        Ok(())
    }

    #[test]
    fn exit_observation_uses_only_the_remaining_admission_budget() -> Result<()> {
        let deadline = Instant::now() + LIFECYCLE_TIMEOUT;
        for remaining in [Duration::ZERO, Duration::from_millis(10)] {
            let clock = Cell::new(deadline - remaining);
            let paused = Cell::new(Duration::ZERO);
            assert!(wait_for_exit_until(
                deadline,
                || Ok(None),
                || clock.get(),
                |delay| {
                    clock.set(clock.get() + delay);
                    paused.set(paused.get() + delay);
                },
            )?
            .is_none());
            assert_eq!(clock.get(), deadline);
            assert_eq!(paused.get(), remaining);
        }
        let error = wait_for_exit_until(
            deadline,
            || anyhow::bail!("child exit observation unavailable"),
            Instant::now,
            |_| panic!("exit observation errors must not be retried"),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("child exit observation unavailable"));
        Ok(())
    }

    #[test]
    fn admission_handles_recorded_cold_store_delay_without_relaunch() -> Result<()> {
        let start = Instant::now();
        let clock = Cell::new(start);
        let polls = Cell::new(0);
        wait_for_readiness(
            42,
            "fixture-pipe",
            start,
            || Ok(()),
            |deadline| {
                polls.set(polls.get() + 1);
                assert!(deadline <= start + LIFECYCLE_TIMEOUT);
                if clock.get() < start + Duration::from_millis(2017) {
                    clock.set(deadline);
                    Err(ClientError::Io {
                        operation: coven_client::WINDOWS_CONNECT_OPERATION,
                        source: std::io::ErrorKind::TimedOut.into(),
                    })
                } else {
                    Ok(Some(healthy()))
                }
            },
            || clock.get(),
            |delay| clock.set(clock.get() + delay),
        )?;
        assert!(clock.get() > start + Duration::from_secs(2));
        assert!(polls.get() > 1);
        Ok(())
    }

    #[test]
    fn admission_exhausts_one_fixed_budget_and_retains_pending_reason() {
        let start = Instant::now();
        let clock = Cell::new(start);
        let error = wait_for_readiness(
            42,
            "fixture-pipe",
            start,
            || Ok(()),
            |deadline| {
                assert!(deadline <= start + LIFECYCLE_TIMEOUT);
                clock.set(deadline);
                Ok(None)
            },
            || clock.get(),
            |delay| clock.set(clock.get() + delay),
        )
        .unwrap_err();
        assert_eq!(clock.get(), start + LIFECYCLE_TIMEOUT);
        assert!(error.to_string().contains("15s"));
        assert!(error.to_string().contains("endpoint not yet available"));
    }

    #[test]
    fn admission_does_not_reset_the_pre_spawn_budget_when_probing_begins() {
        let started = Instant::now();
        let deadline = started + LIFECYCLE_TIMEOUT;
        for remaining in [Duration::ZERO, Duration::from_millis(10)] {
            let clock = Cell::new(deadline - remaining);
            let probes = Cell::new(0);
            let error = wait_for_readiness(
                42,
                "fixture-pipe",
                started,
                || Ok(()),
                |probe_deadline| {
                    probes.set(probes.get() + 1);
                    assert_eq!(probe_deadline, deadline);
                    Ok(None)
                },
                || clock.get(),
                |delay| clock.set(clock.get() + delay),
            )
            .unwrap_err();
            assert_eq!(clock.get(), deadline);
            assert_eq!(probes.get(), usize::from(!remaining.is_zero()));
            assert!(error.to_string().contains("15s"));
        }
    }

    #[test]
    fn admission_rejects_unowned_or_invalid_health_without_retry() {
        for field in ["server_pid", "pid", "socket", "ok", "status", "json"] {
            let mut server_pid = 42;
            let mut status = 200;
            let mut body = json!({"ok": true, "daemon": {"pid": 42, "socket": "fixture-pipe"}});
            match field {
                "server_pid" => server_pid = 7,
                "status" => status = 503,
                "pid" => body["daemon"]["pid"] = json!(7),
                "socket" => body["daemon"]["socket"] = json!("another-pipe"),
                "ok" => body["ok"] = json!(false),
                "json" => {}
                _ => unreachable!(),
            }
            let mut probe = Some(HealthProbe::Windows {
                server_pid,
                status,
                body: if field == "json" {
                    b"{".to_vec()
                } else {
                    serde_json::to_vec(&body).unwrap()
                },
            });
            assert!(
                wait_for_readiness(
                    42,
                    "fixture-pipe",
                    Instant::now(),
                    || Ok(()),
                    |_| Ok(Some(probe.take().expect("must not retry invalid health"))),
                    Instant::now,
                    |_| panic!("invalid health is not pending"),
                )
                .is_err(),
                "{field}"
            );
        }
        #[cfg(unix)]
        for (pid, socket) in [(7, "fixture-pipe"), (42, "another-socket")] {
            let mut probe = Some(HealthProbe::Unix(coven_client::LifecycleDaemonStatus {
                pid,
                socket: socket.to_owned(),
                started_at: "synthetic-start".to_owned(),
            }));
            assert!(wait_for_readiness(
                42,
                "fixture-pipe",
                Instant::now(),
                || Ok(()),
                |_| Ok(Some(
                    probe.take().expect("must not retry mismatched Unix health")
                )),
                Instant::now,
                |_| panic!("Unix identity mismatch is not pending"),
            )
            .is_err());
        }
    }

    #[test]
    fn admission_rejects_child_exit_or_identity_uncertainty_even_after_health() {
        for fail_on in [1, 2] {
            let mut checks = 0;
            let error = wait_for_readiness(
                42,
                "fixture-pipe",
                Instant::now(),
                || {
                    checks += 1;
                    anyhow::ensure!(checks != fail_on, "child identity unavailable");
                    Ok(())
                },
                |_| Ok(Some(healthy())),
                Instant::now,
                |_| panic!("child uncertainty is not pending"),
            )
            .unwrap_err();
            assert!(error.to_string().contains("child identity unavailable"));
        }
    }

    #[test]
    fn admission_rejects_health_returned_after_deadline() {
        let start = Instant::now();
        let clock = Cell::new(start);
        let error = wait_for_readiness(
            42,
            "fixture-pipe",
            start,
            || Ok(()),
            |_| {
                clock.set(start + LIFECYCLE_TIMEOUT);
                Ok(Some(healthy()))
            },
            || clock.get(),
            |_| panic!("late health must not retry"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("exceeded its deadline"));
    }

    #[test]
    fn fatal_probe_preserves_primary_error_and_observes_exit_during_probe() {
        let exited = Cell::new(false);
        let observed_exit = Cell::new(false);
        let probes = Cell::new(0);
        let error = wait_for_readiness(
            42,
            "fixture-pipe",
            Instant::now(),
            || {
                if exited.get() {
                    observed_exit.set(true);
                    anyhow::bail!("fixture child exited with code 17");
                }
                Ok(())
            },
            |_| {
                probes.set(probes.get() + 1);
                exited.set(true);
                Err(ClientError::InvalidHttpResponse(
                    "connection closed before response completed".to_owned(),
                ))
            },
            Instant::now,
            |_| panic!("a fatal probe must not be retried"),
        )
        .unwrap_err();
        assert_eq!(probes.get(), 1);
        assert!(matches!(
            error.downcast_ref::<ClientError>(),
            Some(ClientError::InvalidHttpResponse(message))
                if message == "connection closed before response completed"
        ));
        assert!(
            observed_exit.get(),
            "exit during the fatal probe was not observed"
        );
    }

    #[test]
    fn startup_wait_retries_only_pending_transport_not_identity_or_protocol_errors() {
        assert!(is_pending_startup_error(
            &ClientError::Io {
                operation: coven_client::WINDOWS_CONNECT_OPERATION,
                source: std::io::ErrorKind::TimedOut.into(),
            },
            false
        ));
        assert!(is_pending_startup_error(
            &ClientError::InvalidHttpResponse(coven_client::EMPTY_RESPONSE_TIMEOUT_MESSAGE.into()),
            false,
        ));
        for kind in [
            std::io::ErrorKind::ConnectionRefused,
            std::io::ErrorKind::NotFound,
        ] {
            for unix in [false, true] {
                assert_eq!(
                    is_pending_startup_error(
                        &ClientError::Io {
                            operation: "connecting foreground fixture socket",
                            source: kind.into(),
                        },
                        unix
                    ),
                    unix
                );
            }
        }
        for error in [
            ClientError::DaemonInstanceChanged,
            ClientError::Discovery("wrong owner".into()),
            ClientError::InvalidHttpResponse("partial response timed out".into()),
            ClientError::Io {
                operation: coven_client::WINDOWS_CONNECT_OPERATION,
                source: std::io::ErrorKind::PermissionDenied.into(),
            },
            ClientError::Io {
                operation: "failed to read Coven daemon response",
                source: std::io::Error::from_raw_os_error(109),
            },
        ] {
            for unix in [false, true] {
                assert!(!is_pending_startup_error(&error, unix), "{error}");
            }
            let mut error = Some(error);
            assert!(wait_for_readiness(
                42,
                "fixture-pipe",
                Instant::now(),
                || Ok(()),
                |_| Err(error.take().expect("must not retry fatal probe")),
                Instant::now,
                |_| panic!("fatal transport error is not pending"),
            )
            .is_err());
        }
    }
}
