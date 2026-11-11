//! Native Windows tests for the actual named-pipe stop path. The worker and
//! daemon are real; the test gate withholds exit observation, not termination.
//! These tests require execution on Windows and do not model physical I/O.
#![cfg(windows)]
use super::*;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::process::{Child, Command, Stdio};

const OWNER: &str = "daemon::windows_retained_tests::windows_retained_owner_fixture";
const LEGACY: &str = "daemon::windows_retained_tests::windows_legacy_owner_fixture";

// Keep both identities in one cleanup owner. A local worker guard declared
// after the root would otherwise drop first during unwind and lose the child
// witness before the root lifetime Job initiates its asynchronous termination.
struct OwnedChild(Child, Option<OwnedHandle>);

fn observe_worker_exit(worker: &OwnedHandle, milliseconds: u32) -> std::io::Result<bool> {
    use windows_sys::Win32::{
        Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::WaitForSingleObject,
    };
    match unsafe { WaitForSingleObject(worker.as_raw_handle(), milliseconds) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        WAIT_FAILED => Err(std::io::Error::last_os_error()),
        value => Err(std::io::Error::other(format!(
            "unexpected worker wait result {value}"
        ))),
    }
}

impl OwnedChild {
    fn retain_worker_before_stop(&mut self, marker: &Path) -> std::io::Result<u32> {
        use windows_sys::Win32::System::Threading::OpenProcess;
        let pid = std::fs::read_to_string(marker)?
            .trim()
            .parse::<u32>()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        // The fresh marker comes from the actual root-owned Child. Retain its
        // kernel identity before stopping that root; no PID reopening follows
        // root exit and the handle has no termination or write permission.
        const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
        let handle = unsafe { OpenProcess(SYNCHRONIZE_ACCESS, 0, pid) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        self.1 = Some(unsafe { OwnedHandle::from_raw_handle(handle) });
        if observe_worker_exit(self.1.as_ref().unwrap(), 0)? {
            return Err(std::io::Error::other(
                "native worker exited before the stop fixture",
            ));
        }
        Ok(pid)
    }

    fn wait_worker_exit(&self, timeout: Duration) -> std::io::Result<bool> {
        let worker = self
            .1
            .as_ref()
            .ok_or_else(|| std::io::Error::other("worker HANDLE was not retained"))?;
        let milliseconds = timeout.as_millis().min((u32::MAX - 1) as u128) as u32;
        observe_worker_exit(worker, milliseconds)
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
        if let Some(worker) = self.1.as_ref() {
            // Test failure is not permission to detach authority. The normal
            // explicit wait below is bounded for diagnosis; unwind cleanup
            // keeps this stable witness until actual worker exit is observed.
            let mut reported = false;
            loop {
                match observe_worker_exit(worker, 1000) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(error) => {
                        if !reported {
                            eprintln!("Windows fixture worker cleanup observation failed: {error}");
                            reported = true;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        }
    }
}

fn fixture_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    ensure_private_coven_home(home.path()).unwrap();
    std::fs::create_dir(home.path().join("user")).unwrap();
    home
}

fn spawn_fixture(home: &Path, entry: &str, observation_release: Option<&Path>) -> OwnedChild {
    let user = home.join("user");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", entry, "--ignored", "--nocapture"])
        .env("COVEN_TEST_WINDOWS_RETAINED_HOME", home)
        .env("COVEN_HOME", home)
        .env("HOME", &user)
        .env("USERPROFILE", &user)
        .env("XDG_CONFIG_HOME", user.join(".config"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    if let Some(release) = observation_release {
        command.env("COVEN_TEST_MOBILE_OBSERVATION_RELEASE", release);
    } else {
        command.env_remove("COVEN_TEST_MOBILE_OBSERVATION_RELEASE");
    }
    OwnedChild(command.spawn().unwrap(), None)
}

fn wait_ready(home: &Path, child: &mut OwnedChild, expect_gate: Option<&Path>) -> DaemonStatus {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "Windows owner fixture exited before readiness"
        );
        if let Ok(Some(status)) = read_status(home) {
            if coven_client::probe_windows_daemon_health_with_identity_until(
                &status.socket,
                Instant::now() + Duration::from_millis(100),
            )
            .ok()
            .flatten()
            .is_some_and(|probe| {
                probe.status == 200
                    && probe.server_pid == status.pid
                    && status
                        .process_creation_time
                        .is_some_and(|time| time.get() == probe.process_creation_time)
            }) && expect_gate.is_none_or(|gate| gate.with_extension("worker-pid").exists())
            {
                return status;
            }
        }
        assert!(
            Instant::now() < deadline,
            "Windows owner fixture did not become ready"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_exit(child: &mut OwnedChild) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.0.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "Windows owner stayed live after observation release"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = child.0.wait().unwrap();
}

fn pipe_namespace_is_retained(pipe_name: &str, pid: u32) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{CreateFileW, OPEN_EXISTING},
        System::Pipes::GetNamedPipeServerProcessId,
    };
    let path: Vec<u16> = std::ffi::OsStr::new(&format!(r"\\.\pipe\{pipe_name}"))
        .encode_wide()
        .chain(Some(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0x0010_0000 | 0x0001_0000,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return std::io::Error::last_os_error().raw_os_error() == Some(ERROR_PIPE_BUSY as i32);
    }
    let mut server = 0;
    let observed =
        unsafe { GetNamedPipeServerProcessId(handle, &mut server) } != 0 && server == pid;
    unsafe {
        CloseHandle(handle);
    }
    observed
}

#[test]
#[ignore = "explicit real Windows retained-owner fixture"]
fn windows_retained_owner_fixture() {
    let Some(home) = std::env::var_os("COVEN_TEST_WINDOWS_RETAINED_HOME").map(PathBuf::from) else {
        return;
    };
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

#[test]
fn windows_stop_preserves_owner_until_mobile_exit_is_observed() {
    use crate::mobile_memory::{
        identity,
        registry::DeviceRegistry,
        transport::{storage, test_support::Relay},
    };
    let home = fixture_home();
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
    let release = home.path().join("release-worker-observation");
    let mut child = spawn_fixture(home.path(), OWNER, Some(&release));
    let original = wait_ready(home.path(), &mut child, Some(&release));
    let worker_pid = child
        .retain_worker_before_stop(&release.with_extension("worker-pid"))
        .expect("retain actual native worker HANDLE before stop");
    let started = Instant::now();
    // This is the actual CLI controller, authenticated pipe request, retained
    // root HANDLE and real worker cleanup. Only exit observation is withheld.
    let stopped = stop_background_server(home.path());
    let elapsed = started.elapsed();
    // Select the actual retained-cleanup phase rather than infer it from a
    // caller timeout. This hang guard does not enlarge the stop deadline.
    let retained_deadline = Instant::now() + Duration::from_secs(30);
    while !release.with_extension("retained").exists()
        && child.0.try_wait().unwrap().is_none()
        && Instant::now() < retained_deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    let alive = child.0.try_wait().unwrap().is_none();
    let status = read_status(home.path()).ok().flatten();
    let serve_locked = acquire_serve_lock(home.path()).is_err();
    let profile_locked = crate::state_lock::try_acquire_exclusive(home.path())
        .unwrap()
        .is_none();
    let pipe_retained = pipe_namespace_is_retained(&original.socket, original.pid);
    let retained = std::fs::read_to_string(release.with_extension("retained")).ok();
    // Release and observe the owned process before asserting RED properties.
    // No cleanup thread or live authority is detached by a failing assertion.
    std::fs::write(&release, b"resume actual worker exit observation\n").unwrap();
    wait_exit(&mut child);
    let cleanup_started = Instant::now();
    let worker_exited = child.wait_worker_exit(Duration::from_secs(30));
    assert!(matches!(worker_exited, Ok(true)), "actual native worker {worker_pid} exit remained unobserved during RED cleanup after {:?}: {worker_exited:?}", cleanup_started.elapsed());
    assert!(stopped.is_err(), "Windows stop falsely reported stopped with observation withheld: {stopped:?}; elapsed={elapsed:?}");
    assert!(format!("{:#}", stopped.unwrap_err()).contains("not clearing daemon status"));
    assert!(alive && serve_locked && profile_locked && pipe_retained, "Windows owner/profile/pipe escaped while worker exit was unobserved: alive={alive} serve_locked={serve_locked} profile_locked={profile_locked} pipe={pipe_retained}");
    assert_eq!(
        status,
        Some(original),
        "Windows status changed before worker observation"
    );
    assert!(
        retained.is_some_and(|receipt| receipt.contains("exit-unobserved")),
        "no retained ownership receipt"
    );
    assert!(std::fs::read_to_string(home.path().join("serve-result"))
        .unwrap()
        .starts_with("err:mobile worker shutdown incomplete"));
    assert!(crate::state_lock::try_acquire_exclusive(home.path())
        .unwrap()
        .is_some());
    assert!(acquire_serve_lock(home.path()).is_ok());
    // Windows status has historically been cleared by the authenticated stop
    // caller. A caller that timed out leaves it until a later dead-owner audit.
    stop_background_server(home.path()).unwrap();
    assert!(!daemon_status_path(home.path()).exists());
}

#[test]
#[ignore = "explicit old in-process Windows owner fixture"]
fn windows_legacy_owner_fixture() {
    use interprocess::{
        local_socket::{prelude::*, GenericNamespaced, ListenerOptions},
        os::windows::local_socket::ListenerOptionsExt,
    };
    let Some(home) = std::env::var_os("COVEN_TEST_WINDOWS_RETAINED_HOME").map(PathBuf::from) else {
        return;
    };
    let _profile = crate::state_lock::acquire_shared(&home).unwrap();
    let _serve = acquire_serve_lock(&home).unwrap();
    let name = windows_pipe_name(&home).unwrap();
    let listener = ListenerOptions::new()
        .name(name.clone().to_ns_name::<GenericNamespaced>().unwrap())
        .security_descriptor(owner_only_pipe_security_descriptor().unwrap())
        .create_sync()
        .unwrap();
    let status = DaemonStatus {
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339(),
        socket: name,
        process_creation_time: Some(
            WindowsProcessCreationTime::new(
                coven_client::windows_process_creation_time(std::process::id())
                    .unwrap()
                    .unwrap(),
            )
            .unwrap(),
        ),
    };
    write_status(&home, &status).unwrap();
    for accepted in listener.incoming() {
        let stream = accepted.unwrap();
        let mut reader = BufReader::new(&stream);
        let line = read_http_request_line(&mut reader).unwrap();
        let (method, path) = parse_request_line(&line).unwrap();
        let headers = read_http_headers(&mut reader).unwrap();
        let _body = read_http_body(&mut reader, headers.content_length).unwrap();
        let response = if method == "GET" && path == "/health" {
            crate::api::json_response(200, &serde_json::json!({ "ok": true, "apiVersion": crate::api::COVEN_API_NAMED_VERSION, "covenVersion": "legacy-fixture", "capabilities": { "structuredErrors": true }, "daemon": &status })).unwrap()
        } else {
            crate::api::api_error(
                404,
                "not_found",
                "Old in-process daemon does not implement cooperative shutdown.",
                None,
            )
            .unwrap()
        };
        write_api_response(&mut &stream, &response).unwrap();
        let interprocess::local_socket::Stream::NamedPipe(pipe) = &stream;
        pipe.inner().assume_flushed();
    }
}

#[test]
fn windows_legacy_explicit_unsupported_peer_retains_stop_compatibility() {
    let home = fixture_home();
    let mut child = spawn_fixture(home.path(), LEGACY, None);
    wait_ready(home.path(), &mut child, None);
    let started = Instant::now();
    assert!(stop_background_server(home.path()).unwrap());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "compatible Windows stop exceeded the strict two-second contract: {:?}",
        started.elapsed()
    );
    wait_exit(&mut child);
    assert!(!daemon_status_path(home.path()).exists());
}
