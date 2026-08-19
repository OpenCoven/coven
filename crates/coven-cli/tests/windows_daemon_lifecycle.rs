#![cfg(windows)]

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use interprocess::{
    local_socket::{prelude::*, ConnectOptions, GenericNamespaced},
    ConnectWaitMode,
};

struct DaemonGuard {
    coven_home: PathBuf,
}

struct ExactProcessHandle(windows_sys::Win32::Foundation::HANDLE);

impl ExactProcessHandle {
    fn duplicate(child: &std::process::Child) -> Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::{DuplicateHandle, DUPLICATE_SAME_ACCESS},
            System::Threading::GetCurrentProcess,
        };

        let current = unsafe { GetCurrentProcess() };
        let mut duplicated = std::ptr::null_mut();
        let ok = unsafe {
            DuplicateHandle(
                current,
                child.as_raw_handle(),
                current,
                &mut duplicated,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        anyhow::ensure!(
            ok != 0 && !duplicated.is_null(),
            "failed duplicating exact launcher process handle: {}",
            std::io::Error::last_os_error()
        );
        Ok(Self(duplicated))
    }

    fn terminate(&self) {
        use windows_sys::Win32::System::Threading::TerminateProcess;
        unsafe {
            let _ = TerminateProcess(self.0, 1);
        }
    }

    fn wait_for_exit(&self, timeout: Duration) -> Result<bool> {
        use windows_sys::Win32::{
            Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::WaitForSingleObject,
        };

        let milliseconds = timeout.as_millis().min(u32::MAX as u128) as u32;
        match unsafe { WaitForSingleObject(self.0, milliseconds) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            WAIT_FAILED => Err(std::io::Error::last_os_error())
                .context("failed waiting for exact launcher process handle"),
            result => anyhow::bail!("unexpected launcher process wait result {result}"),
        }
    }
}

impl Drop for ExactProcessHandle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        terminate_recorded_daemon(&self.coven_home);
    }
}

fn assert_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed: status={}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn terminate_process(pid: u32) {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE},
    };
    let process = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if !process.is_null() {
        unsafe {
            let _ = TerminateProcess(process, 1);
            CloseHandle(process);
        }
    }
}

fn terminate_recorded_daemon(coven_home: &Path) {
    let Ok(status) = std::fs::read(coven_home.join("daemon.json")) else {
        return;
    };
    let Ok(status) = serde_json::from_slice::<serde_json::Value>(&status) else {
        return;
    };
    let Some(pid) = status["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
    else {
        return;
    };
    terminate_process(pid);
}

fn captured_output_with_timeout(
    label: &str,
    command: &mut Command,
    coven_home: &Path,
) -> Result<Output> {
    const LAUNCHER_EXIT_TIMEOUT: Duration = Duration::from_secs(15);
    const CAPTURE_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
    const LAUNCHER_POLL_INTERVAL: Duration = Duration::from_millis(100);

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command
        .spawn()
        .with_context(|| format!("failed spawning {label}"))?;
    let launcher = ExactProcessHandle::duplicate(&child)?;
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = result_tx.send(child.wait_with_output());
    });

    let launcher_deadline = Instant::now() + LAUNCHER_EXIT_TIMEOUT;
    loop {
        let poll = launcher_deadline
            .saturating_duration_since(Instant::now())
            .min(LAUNCHER_POLL_INTERVAL);
        match result_rx.recv_timeout(poll) {
            Ok(result) => return result.with_context(|| format!("failed waiting for {label}")),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("{label} output waiter disconnected")
            }
            Err(mpsc::RecvTimeoutError::Timeout) if launcher.wait_for_exit(Duration::ZERO)? => {
                // The exact launcher has exited, so any remaining pipe owner
                // is a descendant. Keep this post-exit deadline strict: a
                // detached daemon inheriting either capture handle is the
                // regression this fixture exists to detect.
                return match result_rx.recv_timeout(CAPTURE_CLOSE_TIMEOUT) {
                    Ok(result) => result.with_context(|| format!("failed waiting for {label}")),
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        anyhow::bail!("{label} output waiter disconnected")
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        terminate_recorded_daemon(coven_home);
                        let _ = result_rx.recv_timeout(CAPTURE_CLOSE_TIMEOUT);
                        anyhow::bail!(
                            "{label} launcher exited but captured stdout/stderr did not close within two seconds; the detached daemon likely inherited a launcher capture handle"
                        )
                    }
                };
            }
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() >= launcher_deadline => {
                terminate_recorded_daemon(coven_home);
                launcher.terminate();
                let _ = result_rx.recv_timeout(CAPTURE_CLOSE_TIMEOUT);
                anyhow::bail!("{label} launcher did not exit within fifteen seconds")
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn wait_until(label: &str, mut predicate: impl FnMut() -> Result<bool>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_error = None;
    while Instant::now() < deadline {
        match predicate() {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => last_error = Some(error),
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    if let Some(error) = last_error {
        anyhow::bail!("timed out waiting for {label}; last error: {error:#}");
    }
    anyhow::bail!("timed out waiting for {label}")
}

fn process_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, WaitForSingleObject},
    };

    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    let handle = unsafe { OpenProcess(SYNCHRONIZE_ACCESS, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let result = unsafe { WaitForSingleObject(handle, 0) };
    unsafe { CloseHandle(handle) };
    result == WAIT_TIMEOUT
}

fn legacy_pipe_name(coven_home: &Path) -> String {
    coven_client::supported_windows_pipe_names(coven_home)
        .expect("derive supported Windows daemon pipe names")[2]
        .clone()
}

fn lowercase_hash_pipe_name(coven_home: &Path) -> String {
    use sha2::{Digest, Sha256};

    let canonical = std::fs::canonicalize(coven_home).unwrap_or_else(|_| coven_home.to_path_buf());
    let mut path = canonical.to_string_lossy().replace('/', "\\");
    let mut is_unc = path.starts_with(r"\\");
    if let Some(unc_path) = path.strip_prefix(r"\\?\UNC\") {
        path = format!(r"\\{unc_path}");
        is_unc = true;
    } else if let Some(path_without_prefix) = path.strip_prefix(r"\\?\") {
        path = path_without_prefix.to_owned();
    }
    let mut components = Vec::new();
    if is_unc {
        components.push("unc".to_owned());
    }
    for component in path.split('\\') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            component => components.push(component.to_ascii_lowercase()),
        }
    }
    let mut digest = Sha256::new();
    digest.update(b"coven.daemon.pipe.v1\0");
    digest.update(components.join("\\").as_bytes());
    let digest = digest.finalize();
    let suffix = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("coven-daemon-v1-{suffix}.sock")
}

fn write_inherited_legacy_status(coven_home: &Path, pid: u32, socket: &str) -> Result<()> {
    write_inherited_legacy_status_bytes(
        coven_home,
        &serde_json::to_vec(&serde_json::json!({
            "pid": pid,
            "startedAt": "2026-04-27T10:00:00Z",
            "socket": socket,
        }))?,
    )
}

fn write_inherited_legacy_status_bytes(coven_home: &Path, contents: &[u8]) -> Result<()> {
    std::fs::create_dir_all(coven_home)?;
    coven_client::write_owner_only_windows_daemon_status(coven_home, contents)
        .map_err(anyhow::Error::new)?;
    let status_path = coven_home.join("daemon.json");
    for operation in ["/inheritance:e", "/reset"] {
        let output = Command::new("icacls.exe")
            .arg(&status_path)
            .arg(operation)
            .output()?;
        assert_success(
            &format!("apply inherited ACL to legacy daemon status with {operation}"),
            &output,
        );
    }
    Ok(())
}

fn run_daemon_command(coven_home: &Path, args: &[&str]) -> Result<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_coven"))
        .arg("daemon")
        .args(args)
        .env("COVEN_HOME", coven_home)
        .output()?)
}

fn prepend_path(dir: &Path) -> OsString {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).expect("test PATH must be joinable")
}

fn compile_windows_probe(output: &Path) -> Result<()> {
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/windows_console_probe.rs");
    let compile = Command::new("rustc.exe")
        .args(["--edition=2021", "-o"])
        .arg(output)
        .arg(source)
        .output()?;
    assert_success("compile Windows daemon containment fixture", &compile);
    Ok(())
}

fn prepare_npm_wrapper_fixture(root: &Path) -> Result<PathBuf> {
    let wrapper_dir = root.join("wrapper");
    let wrapper_bin_dir = wrapper_dir.join("bin");
    let wrapper = wrapper_bin_dir.join("coven.js");
    let native_dir = wrapper_dir
        .join("node_modules")
        .join("@opencoven")
        .join("cli-windows");
    let native_bin_dir = native_dir.join("bin");
    std::fs::create_dir_all(&wrapper_bin_dir)?;
    std::fs::create_dir_all(&native_bin_dir)?;
    std::fs::write(
        wrapper_dir.join("package.json"),
        r#"{"name":"@opencoven/cli-test","type":"module"}"#,
    )?;
    std::fs::write(
        native_dir.join("package.json"),
        r#"{"name":"@opencoven/cli-windows","version":"0.0.0"}"#,
    )?;
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .context("coven-cli manifest has no repository root")?
        .to_path_buf();
    std::fs::copy(repo_root.join("npm/coven/bin/coven.js"), &wrapper)?;
    std::fs::copy(
        env!("CARGO_BIN_EXE_coven"),
        native_bin_dir.join("coven.exe"),
    )?;
    Ok(wrapper)
}

fn wait_for_daemon_status(coven_home: &Path, path: &OsString) -> Result<serde_json::Value> {
    let status_path = coven_home.join("daemon.json");
    wait_until("Windows daemon health", || {
        if !status_path.exists() {
            return Ok(false);
        }
        let status = Command::new(env!("CARGO_BIN_EXE_coven"))
            .args(["daemon", "status"])
            .env("COVEN_HOME", coven_home)
            .env("PATH", path)
            .output()?;
        Ok(status.status.success())
    })?;
    let status: serde_json::Value = serde_json::from_slice(&std::fs::read(status_path)?)?;
    let creation_time = status["processCreationTime"]
        .as_str()
        .context("managed Windows daemon status omitted its process creation time")?
        .parse::<u64>()
        .context("managed Windows daemon status had an invalid process creation time")?;
    anyhow::ensure!(
        creation_time != 0,
        "managed Windows daemon status had a zero process creation time"
    );
    Ok(status)
}

fn send_launch_request(
    status: &serde_json::Value,
    project: &Path,
    prompt: &str,
) -> Result<interprocess::local_socket::Stream> {
    let pipe_name = status["socket"]
        .as_str()
        .context("daemon status omitted Windows pipe name")?;
    let name = pipe_name
        .to_ns_name::<GenericNamespaced>()
        .context("invalid Windows daemon pipe name")?;
    let mut stream = ConnectOptions::new()
        .name(name)
        .wait_mode(ConnectWaitMode::Timeout(Duration::from_secs(2)))
        .connect_sync()?;
    let body = serde_json::json!({
        "projectRoot": project,
        "harness": "codex",
        "launchMode": "nonInteractive",
        "prompt": prompt
    })
    .to_string();
    write!(
        stream,
        "POST /sessions HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )?;
    stream.flush()?;
    Ok(stream)
}

#[test]
fn inherited_legacy_status_with_dead_pid_does_not_wedge_lifecycle_commands() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let coven_home = temp.path().join("coven-home");
    std::fs::create_dir_all(&coven_home)?;
    let pipe_name = lowercase_hash_pipe_name(&coven_home);
    let mut exited = Command::new("cmd.exe")
        .args(["/C", "exit", "/B", "0"])
        .spawn()?;
    let exited_pid = exited.id();
    assert!(exited.wait()?.success());
    let _guard = DaemonGuard {
        coven_home: coven_home.clone(),
    };

    write_inherited_legacy_status(&coven_home, exited_pid, &pipe_name)?;
    let status = run_daemon_command(&coven_home, &["status", "--json"])?;
    assert_success("status with dead inherited legacy record", &status);
    let status: serde_json::Value = serde_json::from_slice(&status.stdout)?;
    assert_eq!(status["status"], "stopped");
    assert!(!coven_home.join("daemon.json").exists());

    write_inherited_legacy_status(&coven_home, exited_pid, &pipe_name)?;
    let stop = run_daemon_command(&coven_home, &["stop"])?;
    assert_success("stop with dead inherited legacy record", &stop);
    assert!(!coven_home.join("daemon.json").exists());

    write_inherited_legacy_status(&coven_home, exited_pid, &pipe_name)?;
    let mut start = Command::new(env!("CARGO_BIN_EXE_coven"));
    start
        .args(["daemon", "start"])
        .env("COVEN_HOME", &coven_home);
    let start = captured_output_with_timeout(
        "start after dead inherited legacy record",
        &mut start,
        &coven_home,
    )?;
    assert_success("start after dead inherited legacy record", &start);
    let path = std::env::var_os("PATH").unwrap_or_default();
    let migrated = wait_for_daemon_status(&coven_home, &path)?;
    assert_ne!(migrated["socket"], pipe_name);
    assert!(
        migrated["socket"]
            .as_str()
            .is_some_and(|socket| socket.starts_with("coven-daemon-v2-")),
        "new daemon did not migrate to its filesystem-identity pipe: {migrated}"
    );
    let stop = run_daemon_command(&coven_home, &["stop"])?;
    assert_success("stop daemon after stale start recovery", &stop);

    write_inherited_legacy_status(&coven_home, exited_pid, &pipe_name)?;
    let mut restart = Command::new(env!("CARGO_BIN_EXE_coven"));
    restart
        .args(["daemon", "restart"])
        .env("COVEN_HOME", &coven_home);
    let restart = captured_output_with_timeout(
        "restart after dead inherited legacy record",
        &mut restart,
        &coven_home,
    )?;
    assert_success("restart after dead inherited legacy record", &restart);
    wait_for_daemon_status(&coven_home, &path)?;
    let stop = run_daemon_command(&coven_home, &["stop"])?;
    assert_success("stop daemon after stale restart recovery", &stop);
    Ok(())
}

#[test]
fn inherited_legacy_status_with_live_pid_is_preserved_by_lifecycle_commands() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let coven_home = temp.path().join("coven-home");
    let pipe_name = legacy_pipe_name(&coven_home);
    write_inherited_legacy_status(&coven_home, std::process::id(), &pipe_name)?;

    let status = run_daemon_command(&coven_home, &["status", "--json"])?;
    assert_success("status with live inherited legacy PID", &status);
    let status: serde_json::Value = serde_json::from_slice(&status.stdout)?;
    assert_eq!(status["status"], "stale");

    for command in [&["start"][..], &["stop"][..], &["restart"][..]] {
        let output = run_daemon_command(&coven_home, command)?;
        assert!(
            !output.status.success(),
            "daemon {} unexpectedly replaced a live inherited legacy PID",
            command[0]
        );
        assert!(coven_home.join("daemon.json").exists());
    }
    Ok(())
}

#[test]
fn inherited_legacy_status_rejects_cross_profile_and_arbitrary_records() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let coven_home = temp.path().join("coven-home");
    let other_home = temp.path().join("other-profile");
    let redirected = [
        legacy_pipe_name(&other_home),
        "other-daemon.sock".to_owned(),
    ];

    for pipe_name in redirected {
        write_inherited_legacy_status(&coven_home, u32::MAX, &pipe_name)?;
        let status = run_daemon_command(&coven_home, &["status", "--json"])?;
        assert!(
            !status.status.success(),
            "status accepted redirected inherited record {pipe_name}"
        );
        assert!(coven_home.join("daemon.json").exists());
    }

    let same_profile = legacy_pipe_name(&coven_home);
    write_inherited_legacy_status_bytes(
        &coven_home,
        format!(
            r#"{{"pid":"ambiguous","startedAt":"2026-04-27T10:00:00Z","socket":"{same_profile}"}}"#
        )
        .as_bytes(),
    )?;
    let classification =
        coven_client::read_windows_daemon_status_for_lifecycle(&coven_home).unwrap_err();
    assert!(
        classification
            .to_string()
            .contains("inherited daemon status identity could not be validated"),
        "malformed legacy fixture was not classified through the inherited-ACL path: \
         {classification}"
    );
    let status = run_daemon_command(&coven_home, &["status", "--json"])?;
    assert!(!status.status.success());
    assert!(coven_home.join("daemon.json").exists());
    Ok(())
}

#[test]
fn abrupt_daemon_stop_kills_child_stalled_before_per_session_job_attachment() -> Result<()> {
    eprintln!("[windows-daemon-lifecycle] preparing fixture");
    let temp = tempfile::tempdir()?;
    let coven_home = temp.path().join("coven-home");
    let project = temp.path().join("project");
    let fake_bin = temp.path().join("bin");
    let barrier = temp.path().join("strict-preattach");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&fake_bin)?;

    let fake_codex = fake_bin.join("codex.exe");
    compile_windows_probe(&fake_codex)?;
    eprintln!("[windows-daemon-lifecycle] fixture compiled");

    let path = prepend_path(&fake_bin);
    let _guard = DaemonGuard {
        coven_home: coven_home.clone(),
    };
    let mut start = Command::new(env!("CARGO_BIN_EXE_coven"));
    start
        .args(["daemon", "start"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .env("COVEN_TEST_WINDOWS_STRICT_PREATTACH_BARRIER_DIR", &barrier);
    let start = captured_output_with_timeout(
        "captured native Windows daemon start",
        &mut start,
        &coven_home,
    )?;
    assert_success("start Windows daemon with lifetime Job", &start);
    eprintln!("[windows-daemon-lifecycle] daemon start returned");

    let status = wait_for_daemon_status(&coven_home, &path)?;
    eprintln!("[windows-daemon-lifecycle] daemon health confirmed");
    let launch_stream =
        send_launch_request(&status, &project, "stall before per-session Job attachment")?;
    eprintln!("[windows-daemon-lifecycle] launch request sent");

    wait_until("strict pre-attachment barrier", || {
        Ok(barrier.join("ready").exists())
    })?;
    eprintln!("[windows-daemon-lifecycle] strict pre-attachment barrier reached");
    let child_pid = std::fs::read_to_string(barrier.join("pid"))?
        .trim()
        .parse::<u32>()?;
    assert!(
        process_is_alive(child_pid),
        "suspended fixture child was not alive at the pre-attachment barrier"
    );

    let started = Instant::now();
    let stop = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["daemon", "stop"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .output()?;
    assert_success("abrupt Windows daemon stop", &stop);
    eprintln!("[windows-daemon-lifecycle] daemon stop returned");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "Windows daemon stop exceeded its documented two-second contract"
    );
    wait_until("inherited daemon Job child termination", || {
        Ok(!process_is_alive(child_pid))
    })?;
    drop(launch_stream);
    Ok(())
}

#[test]
fn abrupt_daemon_stop_kills_live_piped_descendants() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let coven_home = temp.path().join("coven-home");
    let project = temp.path().join("project");
    let fake_bin = temp.path().join("bin");
    let descendant_pid_file = temp.path().join("daemon-descendant.pid");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&fake_bin)?;
    compile_windows_probe(&fake_bin.join("codex.exe"))?;
    let wrapper = prepare_npm_wrapper_fixture(temp.path())?;

    let path = prepend_path(&fake_bin);
    let _guard = DaemonGuard {
        coven_home: coven_home.clone(),
    };
    let mut start = Command::new("node.exe");
    start
        .arg(&wrapper)
        .args(["daemon", "start"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .env("COVEN_WINDOWS_HIDE_NATIVE_WINDOW", "1")
        .env(
            "COVEN_TEST_WINDOWS_CODEX_DESCENDANT_PID_FILE",
            &descendant_pid_file,
        );
    let start = captured_output_with_timeout(
        "captured hidden npm-wrapper Windows daemon start",
        &mut start,
        &coven_home,
    )?;
    assert_success("start Windows daemon descendant fixture", &start);
    let status = wait_for_daemon_status(&coven_home, &path)?;
    let launch_stream = send_launch_request(&status, &project, "hold descendant for stop")?;

    wait_until("Windows daemon descendant pid", || {
        Ok(descendant_pid_file.exists())
    })?;
    let descendant_pid = std::fs::read_to_string(&descendant_pid_file)?
        .trim()
        .parse::<u32>()?;
    assert!(process_is_alive(descendant_pid));

    let started = Instant::now();
    let stop = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["daemon", "stop"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .output()?;
    assert_success("abrupt Windows daemon stop with descendant", &stop);
    assert!(started.elapsed() < Duration::from_secs(2));
    wait_until("Windows daemon descendant termination", || {
        Ok(!process_is_alive(descendant_pid))
    })?;
    drop(launch_stream);
    Ok(())
}
