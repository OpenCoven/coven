#[path = "fixtures/daemon_startup_diagnostics.rs"]
mod daemon_startup_diagnostics;

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

const RUNTIME_PHASES: &[&str] = &[
    "runtime-initialize-begin",
    "orphaned-sessions-end",
    "stale-created-sessions-end",
    "afs-recovery-end",
    "session-runtime-end",
    "threads-scheduler-end",
    "maintenance-scheduler-end",
    "liveness-sweeper-end",
    "automations-scheduler-end",
    "mobile-gateway-end",
    "api-accept-loop-enter",
];

struct OwnedChild(Child);

struct StartupFailureReport<'a> {
    home: &'a Path,
    started: Instant,
    complete: bool,
}

impl Drop for StartupFailureReport<'_> {
    fn drop(&mut self) {
        if !self.complete {
            // Constructed before owned children and after TempDir: cleanup
            // runs first, then filtered capture, then fixture removal. This
            // also runs on Result propagation and assertion unwinding.
            let _ = writeln!(
                std::io::stderr(),
                "startup exercise failed elapsed_ms={}\n{}",
                self.started.elapsed().as_millis(),
                daemon_startup_diagnostics::capture_startup_checkpoints(self.home)
            );
        }
    }
}

impl OwnedChild {
    fn terminate(&mut self) -> Result<()> {
        if self.0.try_wait()?.is_none() {
            self.0.kill().context("terminate own foreground daemon")?;
        }
        self.0.wait().context("reap own foreground daemon")?;
        Ok(())
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

fn command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
    command.env_clear();
    for name in ["PATH", "Path", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .args(args)
        .current_dir(root)
        .env("HOME", root.join("user"))
        .env("USERPROFILE", root.join("user"))
        .env("COVEN_HOME", root.join("coven"))
        .env("XDG_CONFIG_HOME", root.join("user/config"))
        .env("XDG_DATA_HOME", root.join("user/data"))
        .env("XDG_CACHE_HOME", root.join("user/cache"))
        .env("TMPDIR", root.join("tmp"))
        .env("TMP", root.join("tmp"))
        .env("TEMP", root.join("tmp"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn wait_exit(child: &mut OwnedChild, deadline: Instant) -> Result<ExitStatus> {
    loop {
        if let Some(status) = child.0.try_wait()? {
            return Ok(status);
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "owned command exceeded test hang guard"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn read_bounded(path: &Path) -> Result<String> {
    let mut text = String::new();
    File::open(path)?
        .take(64 * 1024)
        .read_to_string(&mut text)?;
    Ok(text)
}

fn assert_runtime_phases(home: &Path, expected: &[&str]) -> Result<()> {
    let captured = daemon_startup_diagnostics::capture_startup_checkpoints(home);
    let mut phases = Vec::new();
    let mut previous_elapsed = 0;
    let mut previous_observer = 0;
    for line in captured.lines() {
        let Some(fields) = line.strip_prefix("startup_checkpoint phase=") else {
            continue;
        };
        let fields: Vec<_> = fields.split_whitespace().collect();
        if !RUNTIME_PHASES.contains(&fields[0]) {
            continue;
        }
        phases.push(fields[0]);
        let elapsed = fields[1]
            .strip_prefix("elapsed_ms=")
            .context("elapsed")?
            .parse::<u128>()?;
        let observer = fields[2]
            .strip_prefix("prior_observer_ms=")
            .context("observer")?
            .parse::<u128>()?;
        let interval = fields[3]
            .strip_prefix("interval_us=")
            .context("interval")?
            .parse::<u128>()?;
        assert!(
            elapsed >= previous_elapsed && observer >= previous_observer,
            "{captured}"
        );
        assert!(
            observer <= elapsed && interval <= elapsed * 1000 + 999,
            "{captured}"
        );
        previous_elapsed = elapsed;
        previous_observer = observer;
    }
    assert_eq!(phases, expected, "{captured}");
    Ok(())
}

/// The startup store closes on its own thread once the accept loop runs
/// (#1227), or on the way out of a startup that failed before readiness.
/// Either way both of its close checkpoints follow `preceding`.
fn assert_store_closes_after(home: &Path, preceding: &str, deadline: Instant) -> Result<()> {
    loop {
        let captured = daemon_startup_diagnostics::capture_startup_checkpoints(home);
        let phases: Vec<_> = captured
            .lines()
            .filter_map(|line| line.strip_prefix("startup_checkpoint phase="))
            .filter_map(|fields| fields.split_whitespace().next())
            .collect();
        let position = |phase: &str| phases.iter().position(|seen| *seen == phase);
        if let Some(end) = position("daemon-store-end") {
            let (Some(preceding), Some(begin)) =
                (position(preceding), position("store-close-begin"))
            else {
                anyhow::bail!("startup store close checkpoints incomplete:\n{captured}");
            };
            anyhow::ensure!(
                preceding < begin && begin < end,
                "startup store closed before its readiness boundary:\n{captured}"
            );
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "startup store close exceeded test hang guard:\n{captured}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn exercise_startup(fail_mobile: bool, unavailable_log: bool) -> Result<()> {
    exercise_startup_with(fail_mobile, unavailable_log, |_, _| Ok(()))
}

fn exercise_startup_with(
    fail_mobile: bool,
    unavailable_log: bool,
    after_launch: impl FnOnce(&Path, &mut OwnedChild) -> Result<()>,
) -> Result<()> {
    // Short synthetic paths also fit the Unix socket pathname limit on macOS.
    let temp_base = if cfg!(unix) {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    let temp = tempfile::Builder::new()
        .prefix("coven-phases-")
        .tempdir_in(temp_base)?;
    let root = temp.path().canonicalize()?;
    let home = root.join("coven");
    let mut failure_report = StartupFailureReport {
        home: &home,
        started: Instant::now(),
        complete: false,
    };
    for directory in [&home, &root.join("user"), &root.join("tmp")] {
        fs::create_dir(directory)?;
    }
    if unavailable_log {
        fs::create_dir(home.join("daemon-recovery.log"))?;
    }
    if fail_mobile {
        let mobile = home.join("mobile");
        fs::create_dir(&mobile)?;
        let config = mobile.join("gateway.json");
        fs::write(
            &config,
            r#"{"enabled":true,"bind":"192.168.1.10:0","advertisedEndpoint":"https://192.168.1.10"}"#,
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&mobile, fs::Permissions::from_mode(0o700))?;
            fs::set_permissions(&config, fs::Permissions::from_mode(0o600))?;
        }
    }
    let stderr_path = root.join("serve.stderr");
    let mut daemon = OwnedChild(
        command(&root, &["daemon", "serve"])
            .stderr(File::create(&stderr_path)?)
            .spawn()?,
    );
    after_launch(&home, &mut daemon)?;
    // A harness hang guard, not a startup-performance assertion or a change
    // to Coven's own lifecycle budget.
    let deadline = Instant::now() + Duration::from_secs(30);
    if fail_mobile {
        let status = wait_exit(&mut daemon, deadline)?;
        assert!(
            !status.success(),
            "invalid enabled mobile configuration was accepted"
        );
        assert!(read_bounded(&stderr_path)?.contains("mobile gateway bind port must be non-zero"));
        assert_runtime_phases(&home, &RUNTIME_PHASES[..9])?;
        assert_store_closes_after(&home, "automations-scheduler-end", deadline)?;
    } else {
        loop {
            anyhow::ensure!(
                daemon.0.try_wait()?.is_none(),
                "foreground daemon exited before readiness"
            );
            let output = root.join("status.json");
            let mut probe = OwnedChild(
                command(&root, &["daemon", "status", "--json"])
                    .stdout(File::create(&output)?)
                    .spawn()?,
            );
            let status = wait_exit(&mut probe, deadline)?;
            let result = read_bounded(&output)?;
            if status.success() {
                let status: serde_json::Value = serde_json::from_str(&result)?;
                if status["status"] == "running" {
                    assert_eq!(status["pid"].as_u64(), Some(u64::from(daemon.0.id())));
                    break;
                }
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "authenticated readiness exceeded test hang guard"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        if !unavailable_log {
            assert_runtime_phases(&home, RUNTIME_PHASES)?;
            assert_store_closes_after(&home, "api-accept-loop-enter", deadline)?;
        }
    }
    daemon.terminate()?;
    assert!(
        daemon.0.try_wait()?.is_some(),
        "owned daemon cleanup not confirmed"
    );
    failure_report.complete = true;
    Ok(())
}

#[test]
fn native_startup_checkpoints_cover_initialization_through_authenticated_serving() -> Result<()> {
    exercise_startup(false, false)
}

#[test]
fn native_startup_checkpoints_omit_completion_after_mobile_initialization_failure() -> Result<()> {
    exercise_startup(true, false)
}

#[test]
fn native_startup_checkpoints_remain_advisory_when_log_is_unavailable() -> Result<()> {
    exercise_startup(false, true)
}

struct BackgroundCleanup<'a>(&'a Path);

impl Drop for BackgroundCleanup<'_> {
    fn drop(&mut self) {
        // The normal stop command authenticates the instance in this isolated
        // home; never signal a PID obtained from unauthenticated metadata.
        if let Ok(child) = command(self.0, &["daemon", "stop"]).spawn() {
            let _ = wait_exit(
                &mut OwnedChild(child),
                Instant::now() + Duration::from_secs(30),
            );
        }
    }
}

fn exercise_background_failure(action: &str) -> Result<()> {
    let base = if cfg!(unix) {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    let temp = tempfile::Builder::new()
        .prefix("coven-start-fail-")
        .tempdir_in(base)?;
    let root = temp.path().canonicalize()?;
    let home = root.join("coven");
    for directory in [&home, &root.join("user"), &root.join("tmp")] {
        fs::create_dir(directory)?;
    }
    let mobile = home.join("mobile");
    fs::create_dir(&mobile)?;
    let config = mobile.join("gateway.json");
    let original =
        br#"{"enabled":true,"bind":"192.168.1.10:0","advertisedEndpoint":"https://192.168.1.10"}"#;
    fs::write(&config, original)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&mobile, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600))?;
    }
    let cleanup = BackgroundCleanup(&root);
    let stderr = root.join("start.stderr");
    let mut launch = OwnedChild(
        command(&root, &["daemon", action])
            .stderr(File::create(&stderr)?)
            .spawn()?,
    );
    let exit = wait_exit(&mut launch, Instant::now() + Duration::from_secs(30))?;
    assert!(!exit.success(), "invalid enabled gateway must fail startup");
    assert_eq!(fs::read(&config)?, original);
    let message = read_bounded(&stderr)?;
    for expected in [
        "mobile gateway configuration",
        "mobile/gateway.json",
        "non-zero",
        "coven daemon restart",
    ] {
        assert!(message.contains(expected), "missing {expected}: {message}");
    }
    assert!(!message.contains("192.168.1.10"), "{message}");
    // The failure is reported only once the startup store is closed, so the
    // corrected start below cannot meet this daemon still closing it.
    let log = fs::read_to_string(home.join("daemon-recovery.log"))?;
    let closed = log
        .find("phase=daemon-store-end")
        .context("startup store closed")?;
    let reported = log
        .find("startup_failure ")
        .context("startup failure recorded")?;
    assert!(
        closed < reported,
        "startup failure reported before its store closed"
    );

    // Explicit operator correction, retaining the rejected file for recovery.
    let rejected = mobile.join("gateway.rejected.json");
    fs::rename(&config, &rejected)?;
    let mut launch = OwnedChild(command(&root, &["daemon", action]).spawn()?);
    assert!(wait_exit(&mut launch, Instant::now() + Duration::from_secs(30))?.success());
    let output = root.join("status.json");
    let mut probe = OwnedChild(
        command(&root, &["daemon", "status", "--json"])
            .stdout(File::create(&output)?)
            .spawn()?,
    );
    assert!(wait_exit(&mut probe, Instant::now() + Duration::from_secs(30))?.success());
    let status: serde_json::Value = serde_json::from_str(&read_bounded(&output)?)?;
    assert_eq!(status["status"], "running");
    assert!(status["pid"].as_u64().is_some_and(|pid| pid > 0));
    assert_eq!(fs::read(rejected)?, original);
    drop(cleanup);
    #[cfg(unix)]
    assert!(!home.join("coven.sock").exists());
    Ok(())
}

#[test]
fn background_start_reports_configuration_correction_and_recovers() -> Result<()> {
    exercise_background_failure("start")
}

#[test]
fn background_restart_reports_configuration_correction_and_recovers() -> Result<()> {
    exercise_background_failure("restart")
}

// Invoked in a child test process so the parent can inspect the exact failure
// report produced by both Result propagation and assertion unwinding.
#[test]
fn startup_failure_report_fixture() -> Result<()> {
    let Ok(mode) = std::env::var("COVEN_TEST_STARTUP_FAILURE_REPORT") else {
        return Ok(());
    };
    exercise_startup_with(true, false, |home, daemon| {
        let status = wait_exit(daemon, Instant::now() + Duration::from_secs(30))?;
        assert!(!status.success(), "invalid mobile fixture should exit");
        // The daemon has exited and been reaped before this injected failure.
        // Include arbitrary text to prove that failure output uses the filter.
        writeln!(
            fs::OpenOptions::new()
                .append(true)
                .open(home.join("daemon-recovery.log"))?,
            "[synthetic] private-marker-must-stay-out-of-failure-report"
        )?;
        if mode == "unwind" {
            panic!("synthetic assertion failure before phase validation");
        }
        anyhow::bail!("synthetic early error before phase validation")
    })
}

#[test]
fn early_errors_and_unwinds_report_only_bounded_startup_checkpoints() -> Result<()> {
    let temp = tempfile::tempdir()?;
    // Windows creates the nested fixture beneath the isolated TEMP/TMP path.
    // Unix uses /tmp directly, so it cannot expose this missing-directory error.
    fs::create_dir(temp.path().join("tmp"))?;
    for mode in ["early-error", "unwind"] {
        let output = temp.path().join(format!("{mode}.stderr"));
        let child = command(temp.path(), &[]);
        // Reuse the isolated environment, replacing only the executable/args.
        // Command has no program setter, so carry its explicit environment over.
        let mut fixture = Command::new(std::env::current_exe()?);
        fixture
            .env_clear()
            .envs(
                child
                    .get_envs()
                    .filter_map(|(key, value)| value.map(|value| (key, value))),
            )
            .env("COVEN_TEST_STARTUP_FAILURE_REPORT", mode)
            .args(["--exact", "startup_failure_report_fixture", "--nocapture"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(&output)?);
        let mut fixture = OwnedChild(fixture.spawn()?);
        let status = wait_exit(&mut fixture, Instant::now() + Duration::from_secs(30))?;
        assert!(!status.success(), "injected fixture must still fail");
        let report = read_bounded(&output)?;
        assert!(
            report.contains("startup exercise failed elapsed_ms="),
            "{report}"
        );
        assert!(
            report.contains("startup_checkpoint phase=automations-scheduler-end"),
            "{report}"
        );
        assert!(!report.contains("phase=mobile-gateway-end"), "{report}");
        assert!(
            !report.contains("private-marker-must-stay-out-of-failure-report"),
            "{report}"
        );
    }
    Ok(())
}
