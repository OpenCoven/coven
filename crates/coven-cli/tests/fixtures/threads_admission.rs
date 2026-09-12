//! Native authority-journey admission, separate from the production launcher SLA.

use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
#[cfg(any(windows, test))]
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
    exit_status: Option<ExitStatus>,
    ready: bool,
}

impl OwnedDaemon {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        Ok(Self {
            child: command.spawn().context("spawning owned fixture daemon")?,
            exit_status: None,
            ready: false,
        })
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.exit_status
    }

    fn observe_exit(&mut self) -> Result<Option<ExitStatus>> {
        if self.exit_status.is_none() {
            self.exit_status = self.child.try_wait()?;
        }
        Ok(self.exit_status)
    }

    #[cfg(any(windows, test))]
    pub fn ensure_running(&mut self) -> Result<()> {
        if let Some(status) = self.observe_exit()? {
            anyhow::bail!("fixture daemon exited before readiness: {status}");
        }
        Ok(())
    }

    pub fn mark_ready(&mut self) -> Result<()> {
        self.ensure_running()?;
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
            || self.ensure_running(),
            |deadline| {
                coven_client::probe_windows_daemon_health_with_identity_until(&pipe, deadline).map(
                    |probe| {
                        probe.map(|probe| HealthProbe {
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

    pub fn reap(&mut self, terminate: bool) -> Result<()> {
        if self.observe_exit()?.is_some() {
            return Ok(());
        }
        if terminate {
            self.child
                .kill()
                .context("terminating owned fixture daemon")?;
        }
        let started = Instant::now();
        while self.observe_exit()?.is_none() {
            anyhow::ensure!(
                started.elapsed() < LIFECYCLE_TIMEOUT,
                "owned fixture daemon {} was not reaped after {:?}",
                self.id(),
                started.elapsed(),
            );
            thread::sleep(Duration::from_millis(25));
        }
        Ok(())
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

#[cfg(any(windows, test))]
struct HealthProbe {
    server_pid: u32,
    status: u16,
    body: Vec<u8>,
}

#[cfg(any(windows, test))]
fn wait_for_readiness(
    pid: u32,
    pipe: &str,
    mut check_child: impl FnMut() -> Result<()>,
    mut probe: impl FnMut(Instant) -> Result<Option<HealthProbe>, ClientError>,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(Duration),
) -> Result<()> {
    let started = now();
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
                let body: Value = serde_json::from_slice(&probe.body)?;
                anyhow::ensure!(
                    probe.status == 200 && body["ok"] == true,
                    "fixture daemon returned invalid health: HTTP {} {body}",
                    probe.status,
                );
                anyhow::ensure!(
                    probe.server_pid == pid
                        && body["daemon"]["pid"] == pid
                        && body["daemon"]["socket"] == pipe,
                    "fixture health did not identify its owned process and pipe: server_pid={} {body}",
                    probe.server_pid,
                );
                check_child()?;
                let observed = now();
                anyhow::ensure!(
                    observed < deadline,
                    "fixture daemon readiness exceeded its deadline after {:?}",
                    observed.saturating_duration_since(started),
                );
                return Ok(());
            }
            Ok(None) => last_pending = Some("pipe not yet available".to_owned()),
            Err(error) if is_pending_windows_startup_error(&error) => {
                last_pending = Some(error.to_string());
            }
            Err(error) => return Err(error.into()),
        }
        pause(Duration::from_millis(25).min(deadline.saturating_duration_since(now())));
    }
}

#[cfg(any(windows, test))]
fn is_pending_windows_startup_error(error: &ClientError) -> bool {
    match error {
        ClientError::Io { source, .. } => source.kind() == std::io::ErrorKind::TimedOut,
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

    fn healthy() -> HealthProbe {
        HealthProbe {
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
    fn admission_handles_recorded_cold_store_delay_without_relaunch() -> Result<()> {
        let start = Instant::now();
        let clock = Cell::new(start);
        let polls = Cell::new(0);
        wait_for_readiness(
            42,
            "fixture-pipe",
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
        assert!(error.to_string().contains("pipe not yet available"));
    }

    #[test]
    fn admission_rejects_unowned_or_invalid_health_without_retry() {
        for field in ["server_pid", "pid", "socket", "ok", "status", "json"] {
            let mut probe = healthy();
            match field {
                "server_pid" => probe.server_pid = 7,
                "status" => probe.status = 503,
                "json" => probe.body = b"{".to_vec(),
                _ => {
                    let mut body: Value = serde_json::from_slice(&probe.body).unwrap();
                    match field {
                        "pid" => body["daemon"]["pid"] = json!(7),
                        "socket" => body["daemon"]["socket"] = json!("another-pipe"),
                        "ok" => body["ok"] = json!(false),
                        _ => unreachable!(),
                    }
                    probe.body = serde_json::to_vec(&body).unwrap();
                }
            }
            let mut probe = Some(probe);
            assert!(
                wait_for_readiness(
                    42,
                    "fixture-pipe",
                    || Ok(()),
                    |_| Ok(Some(probe.take().expect("must not retry invalid health"))),
                    Instant::now,
                    |_| panic!("invalid health is not pending"),
                )
                .is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn admission_rejects_child_exit_or_identity_uncertainty_even_after_health() {
        for fail_on in [1, 2] {
            let mut checks = 0;
            let error = wait_for_readiness(
                42,
                "fixture-pipe",
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
    fn startup_wait_retries_only_pending_transport_not_identity_or_protocol_errors() {
        assert!(is_pending_windows_startup_error(&ClientError::Io {
            operation: coven_client::WINDOWS_CONNECT_OPERATION,
            source: std::io::ErrorKind::TimedOut.into(),
        }));
        assert!(is_pending_windows_startup_error(
            &ClientError::InvalidHttpResponse(coven_client::EMPTY_RESPONSE_TIMEOUT_MESSAGE.into())
        ));
        for error in [
            ClientError::DaemonInstanceChanged,
            ClientError::Discovery("wrong owner".into()),
            ClientError::InvalidHttpResponse("partial response timed out".into()),
            ClientError::Io {
                operation: coven_client::WINDOWS_CONNECT_OPERATION,
                source: std::io::ErrorKind::PermissionDenied.into(),
            },
        ] {
            assert!(!is_pending_windows_startup_error(&error), "{error}");
            let mut error = Some(error);
            assert!(wait_for_readiness(
                42,
                "fixture-pipe",
                || Ok(()),
                |_| Err(error.take().expect("must not retry fatal probe")),
                Instant::now,
                |_| panic!("fatal transport error is not pending"),
            )
            .is_err());
        }
    }
}
