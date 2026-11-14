//! Test-first panic-hook metadata regression. Include from daemon.rs as:
//! #[cfg(all(test, unix))] #[path="daemon_panic_retained_tests.rs"] mod panic_retained_tests;
//! This first test proves real serve-lock/ShutdownGuard ownership across the
//! panic hook; the retained-native-worker extension is described separately.
#![cfg(unix)]
use super::*;
use std::process::{Child, Command, Stdio};

const ENTRY: &str = "daemon::panic_retained_tests::panic_hook_process_fixture";

#[test]
#[ignore = "explicit panic-hook child fixture"]
fn panic_hook_process_fixture() {
    let Some(home) = std::env::var_os("COVEN_TEST_PANIC_OWNER_HOME").map(PathBuf::from) else {
        return;
    };
    let serve_lock = acquire_serve_lock(&home).unwrap();
    let socket_path = daemon_socket_path(&home);
    let status_path = daemon_status_path(&home);
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    let status = DaemonStatus {
        pid: std::process::id(),
        started_at: "2026-11-11T00:00:00Z".into(),
        socket: socket_path.to_string_lossy().into_owned(),
        process_creation_time: None,
    };
    std::fs::write(&status_path, serde_json::to_vec(&status).unwrap()).unwrap();
    let shutdown_guard = ShutdownGuard {
        socket_path: socket_path.clone(),
        status_path: status_path.clone(),
        pid: status.pid,
    };
    install_daemon_panic_hook(&home);
    let caught = std::panic::catch_unwind(|| panic!("intentional test panic before owner cleanup"));
    assert!(caught.is_err());
    // The live owner deliberately has not dropped either real guard yet.
    std::fs::write(
        home.join("panic-hook-ran"),
        b"hook completed; ownership remains\n",
    )
    .unwrap();
    while !home.join("release-owner").exists() {
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(listener);
    drop(shutdown_guard);
    drop(serve_lock);
    std::process::exit(0);
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

/// FIRST RED: current panic hook unlinks status/socket even though the real
/// serve lock and ShutdownGuard are still owned by the live child process.
#[test]
fn panic_hook_preserves_metadata_until_real_shutdown_owner_cleanup() {
    let home = tempfile::Builder::new()
        .prefix("coven-ph-")
        .tempdir_in("/tmp")
        .unwrap();
    let user = home.path().join("user");
    std::fs::create_dir(&user).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", ENTRY, "--ignored", "--nocapture"])
        .env("COVEN_TEST_PANIC_OWNER_HOME", home.path())
        .env("COVEN_HOME", home.path())
        .env("HOME", &user)
        .env("USERPROFILE", &user)
        .env("XDG_CONFIG_HOME", user.join(".config"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut child = OwnedChild(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(30);
    while !home.path().join("panic-hook-ran").exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "panic-hook fixture exited early"
        );
        assert!(Instant::now() < deadline, "panic hook did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    let alive = child.0.try_wait().unwrap().is_none();
    let socket = daemon_socket_path(home.path()).exists();
    let status = daemon_status_file_pid(&daemon_status_path(home.path()));
    let serve_locked = acquire_serve_lock(home.path()).is_err();
    // Always release and observe owner exit BEFORE the red assertions, so a
    // failing regression does not leave a profile owner or authority process.
    std::fs::write(home.path().join("release-owner"), b"release\n").unwrap();
    assert!(child.0.wait().unwrap().success());
    assert!(
        alive && serve_locked,
        "fixture did not retain real live serve ownership"
    );
    assert!(
        socket,
        "panic hook unlinked a live owner's socket before ShutdownGuard"
    );
    assert_eq!(
        status,
        Some(child.0.id()),
        "panic hook removed active status before owner cleanup"
    );
    assert!(!daemon_socket_path(home.path()).exists());
    assert!(!daemon_status_path(home.path()).exists());
}

#[test]
#[ignore = "explicit real-daemon retained-mobile fixture"]
fn retained_mobile_daemon_process_fixture() {
    let Some(home) = std::env::var_os("COVEN_TEST_RETAINED_DAEMON_HOME").map(PathBuf::from) else {
        return;
    };
    // Normal CLI main owns this lease outside serve_forever. Keep the same
    // real lease while the fixture invokes the actual daemon implementation.
    let _profile = crate::state_lock::acquire_shared(&home).unwrap();
    let result = serve_forever(&home, chrono::Utc::now().to_rfc3339(), None, &[]);
    std::fs::write(
        home.join("serve-result"),
        match result {
            Ok(()) => "ok".to_owned(),
            Err(error) => format!("err:{error:#}"),
        },
    )
    .unwrap();
    std::process::exit(0);
}

/// Requires the cfg(test)-only observation gate in retained_observation_seam.md.
/// Native gateway, strict child, profile lease, listener, scheduler and stop
/// controller are real. Only observation is withheld; kill never reports a
/// fabricated success or exit status.
#[test]
fn unobserved_mobile_exit_keeps_real_daemon_owned_and_stop_reports_failure() {
    use crate::mobile_memory::{
        identity,
        registry::DeviceRegistry,
        transport::{storage, test_support::Relay},
    };
    const CHILD: &str = "daemon::panic_retained_tests::retained_mobile_daemon_process_fixture";
    let home = tempfile::Builder::new()
        .prefix("coven-ro-")
        .tempdir_in("/tmp")
        .unwrap();
    let user = home.path().join("user");
    std::fs::create_dir(&user).unwrap();
    identity::load_or_create_host_identity(home.path(), "192.0.2.1").unwrap();
    DeviceRegistry::load(home.path()).unwrap();
    let relay = Relay::start();
    storage::RelayConfig {
        listener_limit: storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![relay.configuration()],
    }
    .save(home.path())
    .unwrap();
    let release = home.path().join("release-exit-observation");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", CHILD, "--ignored", "--nocapture"])
        .env("COVEN_TEST_RETAINED_DAEMON_HOME", home.path())
        .env("COVEN_TEST_MOBILE_OBSERVATION_RELEASE", &release)
        .env("COVEN_HOME", home.path())
        .env("HOME", &user)
        .env("USERPROFILE", &user)
        .env("XDG_CONFIG_HOME", user.join(".config"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut child = OwnedChild(command.spawn().unwrap());
    let startup_deadline = Instant::now() + Duration::from_secs(30);
    let original = loop {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "daemon exited before readiness"
        );
        if let Some(status) = read_status(home.path()).ok().flatten() {
            if unix_daemon_health_matches_status(home.path(), &status, Duration::from_millis(100))
                .unwrap_or(false)
                && release.with_extension("worker-pid").exists()
            {
                break status;
            }
        }
        assert!(
            Instant::now() < startup_deadline,
            "real daemon never became ready"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    let started = Instant::now();
    let stopped = stop_background_server(home.path());
    let elapsed = started.elapsed();
    let error = stopped.as_ref().err().map(|error| format!("{error:#}"));
    let alive = child.0.try_wait().unwrap().is_none();
    let status = read_status(home.path()).ok().flatten();
    let socket = daemon_socket_path(home.path()).exists();
    let listener = std::os::unix::net::UnixStream::connect(daemon_socket_path(home.path())).is_ok();
    let serve_locked = acquire_serve_lock(home.path()).is_err();
    let profile_locked = crate::state_lock::try_acquire_exclusive(home.path())
        .unwrap()
        .is_none();
    let retained = std::fs::read_to_string(release.with_extension("retained")).ok();
    // Release the observation seam, then require actual OS observation/reap and
    // protocol joins before the real daemon removes listener/status/serve lock.
    std::fs::write(&release, b"resume real exit observation\n").unwrap();
    let finish_deadline = Instant::now() + Duration::from_secs(30);
    while child.0.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < finish_deadline,
            "daemon retained ownership even after observation resumed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(child.0.wait().unwrap().success());
    assert!(
        error
            .as_deref()
            .is_some_and(|error| error.contains("not clearing daemon status")),
        "stop falsely succeeded or lost retained ownership: {stopped:?}; observed {elapsed:?}"
    );
    assert!(
        alive && serve_locked && profile_locked,
        "real daemon ownership escaped during incomplete observation"
    );
    assert!(
        socket && listener,
        "stopping daemon lost its actual socket FD/path before retained cleanup"
    );
    let status = status.expect("status removed before retained cleanup");
    assert_eq!(status.pid, original.pid);
    assert_eq!(status.started_at, original.started_at);
    assert!(
        retained
            .as_deref()
            .is_some_and(|receipt| receipt.contains("exit-unobserved")),
        "missing retained ownership receipt: {retained:?}"
    );
    assert!(!daemon_socket_path(home.path()).exists());
    assert!(!daemon_status_path(home.path()).exists());
    assert!(crate::state_lock::try_acquire_exclusive(home.path())
        .unwrap()
        .is_some());
    assert!(std::fs::read_to_string(home.path().join("serve-result"))
        .unwrap()
        .starts_with("err:mobile worker shutdown incomplete"));
}
