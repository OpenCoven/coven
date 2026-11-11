//! macOS-native parent-death/shared-profile-lease test. Include as a child of
//! gateway::process with cfg(all(test,target_os="macos")); see bridge notes.
//! This kills the ACTUAL parent, not merely its pipe handle in a live process.
#![cfg(target_os = "macos")]
use super::*;
use std::path::PathBuf;

const OWNER: &str =
    "mobile_memory::gateway::process::parent_death_tests::native_owner_process_fixture";

#[test]
#[ignore = "explicit owning-parent process fixture"]
fn native_owner_process_fixture() {
    let Some(home) = std::env::var_os("COVEN_TEST_NATIVE_OWNER_HOME").map(PathBuf::from) else {
        return;
    };
    let _retained = RetainedOwnerGuard;
    let runtime =
        native_fixture::start_fixture(&home, None, None, &home.join("unused-owner-write-ready"))
            .unwrap()
            .unwrap();
    // Open the owner lease AFTER exec: it cannot be inherited by the worker.
    let _profile = crate::state_lock::acquire_shared(&home).unwrap();
    let pid = runtime
        .owned
        .as_ref()
        .unwrap()
        .process
        .lock()
        .unwrap()
        .child
        .id();
    publish_test_observation(&home.join("owner-ready"), pid.to_string().as_bytes()).unwrap();
    let _runtime = runtime; // Retain stdin writer and shared lease until SIGKILL.
    loop {
        std::thread::park();
    }
}

struct OwnedParent {
    child: Child,
    tree: crate::pty_runner::StrictChildProcessTree,
    reaped: bool,
}
impl OwnedParent {
    fn kill_and_reap(&mut self) {
        if self.reaped {
            return;
        }
        self.tree.terminate(&mut self.child);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !crate::pty_runner::poll_child_exit_without_reaping(&self.child).unwrap() {
            assert!(Instant::now() < deadline, "owning parent exit unobserved");
            std::thread::sleep(POLL);
        }
        self.tree.terminate_tree().unwrap();
        let status = self.child.wait().unwrap();
        self.reaped = true;
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            status.signal(),
            Some(libc::SIGKILL),
            "owner did not die by actual SIGKILL"
        );
    }
}
impl Drop for OwnedParent {
    fn drop(&mut self) {
        self.kill_and_reap();
    }
}

struct EofReleaseOnDrop {
    release: PathBuf,
    worker_pid: Option<u32>,
    observed_exit: bool,
}
impl Drop for EofReleaseOnDrop {
    fn drop(&mut self) {
        let _ = publish_test_observation(&self.release, b"cleanup releases synthetic EOF delay\n");
        if self.observed_exit {
            return;
        }
        let Some(pid) = self.worker_pid else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match crate::pty_runner::test_macos_process_has_exited(pid) {
                Ok(true) => return,
                Ok(false) if Instant::now() < deadline => std::thread::sleep(POLL),
                result => {
                    // Never print a successful cleanup receipt for an
                    // unobserved orphan. The failing test retains its PID and
                    // profile path as the concrete investigation boundary.
                    eprintln!("native parent-death cleanup unobserved: pid={pid}, profile={}, observation={result:?}",self.release.display());
                    return;
                }
            }
        }
    }
}

fn wait_marker(path: &Path, parent: &mut OwnedParent) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            !crate::pty_runner::poll_child_exit_without_reaping(&parent.child).unwrap(),
            "owner died before native child readiness"
        );
        assert!(
            Instant::now() < deadline,
            "native marker never published: {}",
            path.display()
        );
        std::thread::sleep(POLL);
    }
}

#[test]
fn actual_parent_death_keeps_child_profile_lease_until_worker_exit() {
    use crate::mobile_memory::{
        identity,
        registry::DeviceRegistry,
        transport::{storage, test_support::Relay},
    };
    let home = tempfile::Builder::new()
        .prefix("coven-pd-")
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
    let release = home.path().join("release-parent-eof");
    // Declared before OwnedParent: unwind kills/reaps the owner first, then
    // releases the native child's EOF gate and observes its actual exit.
    let mut eof_release = EofReleaseOnDrop {
        release: release.clone(),
        worker_pid: None,
        observed_exit: false,
    };
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", OWNER, "--ignored", "--nocapture"])
        .env("COVEN_TEST_NATIVE_OWNER_HOME", home.path())
        .env("COVEN_TEST_MOBILE_PARENT_EOF_RELEASE", &release)
        .env("COVEN_HOME", home.path())
        .env("HOME", &user)
        .env("USERPROFILE", &user)
        .env("XDG_CONFIG_HOME", user.join(".config"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let (child, tree) = crate::pty_runner::spawn_strict_child_process_tree(&mut command).unwrap();
    let mut parent = OwnedParent {
        child,
        tree,
        reaped: false,
    };
    wait_marker(&home.path().join("owner-ready"), &mut parent);
    let worker_pid = std::fs::read_to_string(home.path().join("owner-ready"))
        .unwrap()
        .parse::<u32>()
        .unwrap();
    eof_release.worker_pid = Some(worker_pid);
    assert!(!crate::pty_runner::test_macos_process_has_exited(worker_pid).unwrap());
    assert!(crate::state_lock::try_acquire_exclusive(home.path())
        .unwrap()
        .is_none());
    // SIGKILL bypasses owner Drop and releases its profile lease/pipe writer.
    // The real mobile worker owns a DISTINCT strict session; only EOF will
    // terminate it. The test-only EOF gate selects that scheduling window.
    parent.kill_and_reap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !release.with_extension("eof-seen").exists() {
        assert!(
            Instant::now() < deadline,
            "native child never observed actual parent EOF"
        );
        std::thread::sleep(POLL);
    }
    let child_still_alive = !crate::pty_runner::test_macos_process_has_exited(worker_pid).unwrap();
    let reset_excluded = crate::state_lock::try_acquire_exclusive(home.path())
        .unwrap()
        .is_none();
    // Always release the EOF scheduling seam before asserting, then observe
    // real kernel NOTE_EXIT/ESRCH through the existing tested macOS observer.
    publish_test_observation(&release, b"allow real parent-death process exit\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !crate::pty_runner::test_macos_process_has_exited(worker_pid).unwrap() {
        assert!(
            Instant::now() < deadline,
            "orphaned native mobile worker exit unobserved"
        );
        std::thread::sleep(POLL);
    }
    eof_release.observed_exit = true;
    let reset_after_exit = crate::state_lock::try_acquire_exclusive(home.path())
        .unwrap()
        .is_some();
    assert!(
        child_still_alive,
        "EOF gate did not select a live worker after actual parent death"
    );
    assert!(
        reset_excluded,
        "exclusive reset acquired while orphaned mobile authority still lived"
    );
    assert!(
        reset_after_exit,
        "worker exit did not release its independent profile lease"
    );
}
