//! Real daemon SIGTERM while native child migration is held before Ready.
//! Include in daemon.rs with cfg(all(test,unix)) and this module name:
//! startup_mobile_stop_tests. Requires only the scoped test start hook below.
#![cfg(unix)]
use super::*;
use std::process::{Child, Command, Stdio};

const FIXTURE: &str = "daemon::startup_mobile_stop_tests::startup_sigterm_daemon_fixture";

#[test]
#[ignore = "explicit real startup-SIGTERM daemon fixture"]
fn startup_sigterm_daemon_fixture() {
    let Some(home) = std::env::var_os("COVEN_TEST_STARTUP_STOP_HOME").map(PathBuf::from) else {
        return;
    };
    let _profile = crate::state_lock::acquire_shared(&home).unwrap();
    let result = serve_forever(&home, chrono::Utc::now().to_rfc3339(), None, &[]);
    std::fs::write(
        home.join("startup-return"),
        match result {
            Ok(()) => "ok".to_owned(),
            Err(error) => format!("err:{error:#}"),
        },
    )
    .unwrap();
    std::process::exit(0);
}

struct OwnedDaemon {
    child: Child,
    tree: crate::pty_runner::StrictChildProcessTree,
    reaped: bool,
}
impl OwnedDaemon {
    fn reap(&mut self, kill: bool) {
        if self.reaped {
            return;
        }
        if kill {
            self.tree.terminate(&mut self.child);
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        while !crate::pty_runner::poll_child_exit_without_reaping(&self.child).unwrap() {
            assert!(Instant::now() < deadline, "daemon exit/reap unobserved");
            std::thread::sleep(Duration::from_millis(5));
        }
        self.tree.terminate_tree().unwrap();
        self.child.wait().unwrap();
        self.reaped = true;
    }
}
impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        self.reap(true);
    }
}

#[test]
fn sigterm_during_actual_native_startup_write_reaps_daemon_within_two_seconds() {
    use crate::mobile_memory::{
        config, identity,
        transport::{storage, test_support::Relay},
    };
    let home = tempfile::Builder::new()
        .prefix("coven-su-")
        .tempdir_in("/tmp")
        .unwrap();
    let user = home.path().join("user");
    std::fs::create_dir(&user).unwrap();
    identity::load_or_create_host_identity(home.path(), "192.0.2.1").unwrap();
    // serve_forever uses the canonical home. Match the exact child observer
    // path on macOS, where /tmp is an alias for /private/tmp.
    let canonical_home = home.path().canonicalize().unwrap();
    let mobile = config::ensure_private_mobile_dir(&canonical_home).unwrap();
    let target = mobile.join("devices.json");
    let legacy = serde_json::to_vec(&serde_json::json!({"version":1,"devices":[]})).unwrap();
    config::atomic_replace_private(&target, &legacy).unwrap();
    let relay = Relay::start();
    storage::RelayConfig {
        listener_limit: storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![relay.configuration()],
    }
    .save(home.path())
    .unwrap();
    let ready = home.path().join("startup-write-held");
    let pid_path = home.path().join("startup-worker-pid");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", FIXTURE, "--ignored", "--nocapture"])
        .env("COVEN_TEST_STARTUP_STOP_HOME", home.path())
        .env("COVEN_TEST_DAEMON_STARTUP_HOLD", &target)
        .env("COVEN_TEST_DAEMON_STARTUP_READY", &ready)
        .env("COVEN_TEST_DAEMON_STARTUP_WORKER_PID", &pid_path)
        .env("COVEN_HOME", home.path())
        .env("HOME", &user)
        .env("USERPROFILE", &user)
        .env("XDG_CONFIG_HOME", user.join(".config"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let (child, tree) = crate::pty_runner::spawn_strict_child_process_tree(&mut command).unwrap();
    let mut daemon = OwnedDaemon {
        child,
        tree,
        reaped: false,
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready.exists() {
        assert!(
            !crate::pty_runner::poll_child_exit_without_reaping(&daemon.child).unwrap(),
            "daemon exited before held native startup write"
        );
        assert!(
            Instant::now() < deadline,
            "child never reached initial migration Written"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let marker = std::fs::read(&ready).unwrap();
    let worker_pid = std::fs::read_to_string(&pid_path)
        .unwrap()
        .parse::<u32>()
        .unwrap();
    let before_signal = std::fs::read(&target).unwrap();
    let started = Instant::now();
    assert_eq!(
        unsafe { libc::kill(daemon.child.id() as _, libc::SIGTERM) },
        0
    );
    let stopped = loop {
        if crate::pty_runner::poll_child_exit_without_reaping(&daemon.child).unwrap() {
            break true;
        }
        if started.elapsed() >= Duration::from_secs(2) {
            break false;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    daemon.reap(!stopped);
    let elapsed = started.elapsed();
    let after = std::fs::read(&target).unwrap();
    let returned = std::fs::read_to_string(home.path().join("startup-return")).ok();
    #[cfg(target_os = "macos")]
    let worker_exited = crate::pty_runner::test_macos_process_has_exited(worker_pid).unwrap();
    #[cfg(not(target_os = "macos"))]
    let worker_exited = unsafe { libc::kill(worker_pid as _, 0) } == -1
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    #[cfg(target_os = "macos")]
    let worker_exited = {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut observed = worker_exited;
        while !observed && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            observed = crate::pty_runner::test_macos_process_has_exited(worker_pid).unwrap();
        }
        observed
    };
    // Failure cleanup and real root reap precede assertions; the hold was never
    // released, so it cannot finish migration after an apparent stop success.
    assert_eq!(marker, b"held-written\n");
    assert_eq!(before_signal, legacy);
    assert!(stopped && elapsed<Duration::from_secs(2),"startup SIGTERM outlived two seconds: {elapsed:?}; root_stopped={stopped}; returned={returned:?}");
    assert!(
        worker_exited,
        "daemon returned without observed mobile child exit: pid={worker_pid}"
    );
    assert_eq!(after, legacy, "held startup write committed after shutdown");
    assert!(
        returned
            .as_deref()
            .is_some_and(|text| text.contains("mobile startup cancelled during daemon shutdown")),
        "startup stop was not promptly classified: {returned:?}"
    );
}
