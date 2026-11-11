//! Create-only authority crash/error/race regressions with real native children.
//! These filesystem component tests complement the native gateway process cuts.
use super::*;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const ENTRY: &str = "mobile_memory::config::crash_create_tests::atomic_create_process_fixture";
const FLOOR: &[u8] = b"coven.mobile.routing.v2\n";

fn phase(name: &str) -> MobileWritePhase {
    match name {
        "TempCreated" => MobileWritePhase::TempCreated,
        "Written" => MobileWritePhase::Written,
        "FileSynced" => MobileWritePhase::FileSynced,
        "Renamed" => MobileWritePhase::Renamed,
        "DirectorySynced" => MobileWritePhase::DirectorySynced,
        _ => panic!("unknown create-process phase"),
    }
}

#[test]
#[ignore = "explicit atomic-create crash fixture, exits its process"]
fn atomic_create_process_fixture() {
    let Some(target) = std::env::var_os("COVEN_TEST_CREATE_TARGET").map(PathBuf::from) else {
        return;
    };
    let ready = PathBuf::from(std::env::var_os("COVEN_TEST_CREATE_READY").unwrap());
    let outcome = PathBuf::from(std::env::var_os("COVEN_TEST_CREATE_OUTCOME").unwrap());
    let selected = phase(&std::env::var("COVEN_TEST_CREATE_PHASE").unwrap());
    let error = std::env::var("COVEN_TEST_CREATE_FAULT").as_deref() == Ok("error");
    let observed_target = target.clone();
    set_write_observer(Box::new(move |path, actual| {
        if path != observed_target || actual != selected {
            return Ok(());
        }
        // Publish readiness atomically; existence must mean a complete marker.
        let staged_ready = ready.with_extension("pending");
        fs::write(&staged_ready, format!("{actual:?}\n"))?;
        fs::rename(&staged_ready, &ready)?;
        if error {
            anyhow::bail!("injected create write/sync boundary failure at {actual:?}");
        }
        loop {
            std::thread::park();
        }
    }));
    let result = atomic_create_private(&target, FLOOR);
    let result = match result {
        Ok(created) => format!("ok:{created}"),
        Err(error) => format!("err:{error:#}"),
    };
    fs::write(outcome, result).unwrap();
    std::process::exit(0);
}

struct OwnedFixture {
    child: Child,
    tree: crate::pty_runner::StrictChildProcessTree,
    reaped: bool,
}
impl OwnedFixture {
    fn has_exited(&mut self) -> bool {
        #[cfg(unix)]
        {
            crate::pty_runner::poll_child_exit_without_reaping(&self.child).unwrap()
        }
        #[cfg(not(unix))]
        {
            // Windows containment uses the retained Job HANDLE; observing the
            // leader cannot recycle the containment identity before cleanup.
            self.child.try_wait().unwrap().is_some()
        }
    }

    fn finish(&mut self, terminate: bool) {
        if self.reaped {
            return;
        }
        if terminate {
            self.tree.terminate(&mut self.child);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.has_exited() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "fixture exit unobserved; retain ownership for investigation"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        self.tree.terminate_tree().unwrap();
        self.child.wait().unwrap();
        self.reaped = true;
    }
}
impl Drop for OwnedFixture {
    fn drop(&mut self) {
        if !self.reaped {
            self.finish(true);
        }
    }
}

fn spawn(home: &Path, target: &Path, selected: &str, fault: &str) -> OwnedFixture {
    let user_home = home.join("fixture-user-home");
    fs::create_dir_all(&user_home).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", ENTRY, "--ignored", "--nocapture"])
        .env("HOME", &user_home)
        .env("USERPROFILE", &user_home)
        .env("XDG_CONFIG_HOME", user_home.join(".config"))
        .env("COVEN_HOME", home)
        .env("COVEN_TEST_CREATE_TARGET", target)
        .env("COVEN_TEST_CREATE_READY", home.join("ready"))
        .env("COVEN_TEST_CREATE_OUTCOME", home.join("outcome"))
        .env("COVEN_TEST_CREATE_PHASE", selected)
        .env("COVEN_TEST_CREATE_FAULT", fault)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let (child, tree) = crate::pty_runner::spawn_strict_child_process_tree(&mut command).unwrap();
    OwnedFixture {
        child,
        tree,
        reaped: false,
    }
}

fn wait_checkpoint(child: &mut OwnedFixture, home: &Path, selected: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(bytes) = fs::read(home.join("ready")) {
            assert_eq!(bytes, format!("{selected}\n").as_bytes());
            return;
        }
        assert!(
            !child.has_exited(),
            "fixture exited before selected phase {selected}; outcome={:?}",
            fs::read_to_string(home.join("outcome"))
        );
        assert!(
            Instant::now() < deadline,
            "did not observe create checkpoint {selected}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A killed staging writer must never expose an empty canonical floor.
#[test]
fn create_new_kill_at_temp_created_does_not_publish_an_empty_protocol_floor() {
    let home = tempfile::tempdir().unwrap();
    let mobile = ensure_private_mobile_dir(home.path()).unwrap();
    let target = mobile.join(".transport-v2-00000000-0000-0000-0000-000000000005.required");
    let mut child = spawn(home.path(), &target, "TempCreated", "hold");
    wait_checkpoint(&mut child, home.path(), "TempCreated");
    let published_before_kill = fs::read(&target).ok();
    child.finish(true);
    assert!(!target.exists(),"killed pre-publication create left final authority bytes: before-kill={published_before_kill:?}, after-kill={:?}",fs::read(&target).ok());
    assert!(
        atomic_create_private(&target, FLOOR).unwrap(),
        "fresh recovery should create absent floor"
    );
    assert_eq!(fs::read(&target).unwrap(), FLOOR);
}

#[test]
fn create_new_prepublication_write_and_sync_errors_leave_absent_floor_retryable() {
    for selected in ["TempCreated", "Written", "FileSynced"] {
        let home = tempfile::tempdir().unwrap();
        let mobile = ensure_private_mobile_dir(home.path()).unwrap();
        let target = mobile.join(".transport-v2-test.required");
        let mut child = spawn(home.path(), &target, selected, "error");
        child.finish(false);
        assert_eq!(
            fs::read(home.path().join("ready")).unwrap(),
            format!("{selected}\n").as_bytes()
        );
        let outcome = fs::read_to_string(home.path().join("outcome")).unwrap();
        assert!(
            outcome.starts_with("err:injected create write/sync boundary failure"),
            "{selected}: {outcome}"
        );
        assert!(!target.exists(), "{selected} error published a final floor");
        assert!(atomic_create_private(&target, FLOOR).unwrap());
        assert_eq!(fs::read(&target).unwrap(), FLOOR);
    }
}

#[test]
fn create_new_does_not_replace_or_delete_a_damaged_existing_floor() {
    let home = tempfile::tempdir().unwrap();
    let mobile = ensure_private_mobile_dir(home.path()).unwrap();
    let target = mobile.join(".transport-v2-test.required");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&target)
        .unwrap()
        .write_all(b"damaged-existing-floor")
        .unwrap();
    assert!(!atomic_create_private(&target, FLOOR).unwrap());
    assert_eq!(fs::read(&target).unwrap(), b"damaged-existing-floor");
}

#[test]
fn staged_creators_publish_one_complete_winner_without_clobbering() {
    let home = tempfile::tempdir().unwrap();
    let directory = ensure_private_mobile_dir(home.path()).unwrap();
    let target = directory.join("concurrent-authority");
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(2);
    let outcomes = std::thread::scope(|scope| {
        let mut release = Vec::new();
        let mut workers = Vec::new();
        for value in *b"AB" {
            let target = target.clone();
            let ready = ready_tx.clone();
            let (send, receive) = std::sync::mpsc::sync_channel(1);
            release.push(send);
            workers.push(scope.spawn(move || {
                set_write_observer(Box::new(move |_, phase| {
                    if phase == MobileWritePhase::Written {
                        ready.send(()).unwrap();
                        receive
                            .recv_timeout(Duration::from_secs(30))
                            .context("creator publication barrier never released")?;
                    }
                    Ok(())
                }));
                (value, atomic_create_private(&target, &vec![value; 8192]))
            }));
        }
        let first = ready_rx.recv_timeout(Duration::from_secs(30));
        let second = ready_rx.recv_timeout(Duration::from_secs(30));
        let unpublished = !target.exists();
        // Release and join both producers before any assertion, including a
        // missing checkpoint, so no failed test detaches a held writer.
        for send in release {
            let _ = send.send(());
        }
        let outcomes: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert!(
            first.is_ok() && second.is_ok(),
            "both staged writes were not observed: {first:?}, {second:?}"
        );
        assert!(
            unpublished,
            "a staged writer exposed the final name before publication"
        );
        outcomes
    });
    let winners: Vec<_> = outcomes
        .into_iter()
        .filter_map(|(value, result)| result.unwrap().then_some(value))
        .collect();
    assert_eq!(
        winners.len(),
        1,
        "publication did not select exactly one creator"
    );
    assert_eq!(fs::read(&target).unwrap(), vec![winners[0]; 8192]);
    assert_eq!(
        fs::read_dir(&directory).unwrap().count(),
        1,
        "creator left a disposable staging name"
    );
}

#[test]
#[cfg(unix)]
fn existing_private_read_only_authority_does_not_require_new_write_access() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let directory = ensure_private_mobile_dir(home.path()).unwrap();
    let target = directory.join("read-only-authority");
    assert!(atomic_create_private(&target, b"existing").unwrap());
    fs::set_permissions(&target, fs::Permissions::from_mode(0o400)).unwrap();
    assert!(!atomic_create_private(&target, b"replacement").unwrap());
    assert_eq!(fs::read(&target).unwrap(), b"existing");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o400
    );
}

#[test]
fn create_new_crash_cuts_leave_only_absent_or_complete_authority() {
    for selected in ["Written", "FileSynced", "Renamed", "DirectorySynced"] {
        let home = tempfile::tempdir().unwrap();
        let directory = ensure_private_mobile_dir(home.path()).unwrap();
        let target = directory.join("crash-cut-authority");
        let mut child = spawn(home.path(), &target, selected, "hold");
        wait_checkpoint(&mut child, home.path(), selected);
        child.finish(true);
        let published = matches!(selected, "Renamed" | "DirectorySynced");
        assert_eq!(
            fs::read(&target).ok(),
            published.then(|| FLOOR.to_vec()),
            "invalid canonical image after kill at {selected}"
        );
        assert_eq!(atomic_create_private(&target, FLOOR).unwrap(), !published);
        assert_eq!(fs::read(&target).unwrap(), FLOOR);
    }
}

#[test]
fn create_new_postpublication_errors_keep_complete_authority() {
    for selected in ["Renamed", "DirectorySynced"] {
        let home = tempfile::tempdir().unwrap();
        let directory = ensure_private_mobile_dir(home.path()).unwrap();
        let target = directory.join("post-publication-authority");
        let mut child = spawn(home.path(), &target, selected, "error");
        child.finish(false);
        assert_eq!(
            fs::read(home.path().join("ready")).unwrap(),
            format!("{selected}\n").as_bytes()
        );
        assert!(fs::read_to_string(home.path().join("outcome"))
            .unwrap()
            .starts_with("err:injected create write/sync boundary failure"));
        assert_eq!(fs::read(&target).unwrap(), FLOOR);
        assert!(
            !atomic_create_private(&target, FLOOR).unwrap(),
            "post-publication error removed authority at {selected}"
        );
    }
}
