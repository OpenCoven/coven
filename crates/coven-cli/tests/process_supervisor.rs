use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

const PROTOCOL: &str = "coven.process-supervisor.v1";
const CONTROL_PREFIX: &str = "COVEN_PROCESS_SUPERVISOR_V1 ";

struct ActiveSupervisor {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: ChildStdout,
    stderr: BufReader<ChildStderr>,
}

fn compile_probe(build_dir: &Path) -> Result<PathBuf> {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/process_supervisor_probe.rs");
    let executable = build_dir.join(if cfg!(windows) {
        "process-supervisor-probe.exe"
    } else {
        "process-supervisor-probe"
    });
    let rustc = if cfg!(windows) { "rustc.exe" } else { "rustc" };
    let output = Command::new(rustc)
        .args(["--edition=2021", "-o"])
        .arg(&executable)
        .arg(source)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "failed compiling process-supervisor probe: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(executable)
}

fn process_supervisor_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
    command.args(["process-supervisor", "--protocol", PROTOCOL]);
    command
}

fn launch_supervisor(program: &Path, args: &[String], cwd: &Path) -> Result<ActiveSupervisor> {
    launch_supervisor_with_command(process_supervisor_command(), program, args, cwd)
}

fn launch_supervisor_with_command(
    mut command: Command,
    program: &Path,
    args: &[String],
    cwd: &Path,
) -> Result<ActiveSupervisor> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().context("supervisor stdin")?;
    let request = serde_json::json!({
        "version": 1,
        "program": program,
        "args": args,
        "cwd": cwd,
    });
    serde_json::to_writer(&mut stdin, &request)?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;

    let stdout = child.stdout.take().context("supervisor stdout")?;
    let stderr = child.stderr.take().context("supervisor stderr")?;
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut stderr = BufReader::new(stderr);
        let mut control = String::new();
        let result = stderr.read_line(&mut control).map(|_| (stderr, control));
        let _ = ready_tx.send(result);
    });
    let (stderr, control) = match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(result) => result?,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow::Error::new(error).context("supervisor did not publish ready"));
        }
    };
    assert!(control.starts_with(CONTROL_PREFIX), "{control:?}");
    let event: serde_json::Value = serde_json::from_str(&control[CONTROL_PREFIX.len()..])?;
    assert_eq!(event["event"], "ready", "{event}");
    assert_eq!(event["protocol"], PROTOCOL, "{event}");
    Ok(ActiveSupervisor {
        child,
        stdin: Some(stdin),
        stdout,
        stderr,
    })
}

fn wait_bounded(child: &mut Child, timeout: Duration) -> Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        anyhow::ensure!(Instant::now() < deadline, "supervisor did not exit in time");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_pid(path: &Path) -> Result<u32> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(pid) = raw.trim().parse::<u32>() {
                return Ok(pid);
            }
        }
        anyhow::ensure!(Instant::now() < deadline, "fixture did not publish a pid");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    let handle = unsafe {
        OpenProcess(
            SYNCHRONIZE_ACCESS | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return false;
    }
    unsafe { CloseHandle(handle) };
    true
}

fn wait_for_process_exit(pid: u32) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_alive(pid) {
        anyhow::ensure!(
            Instant::now() < deadline,
            "supervisor left descendant {pid} alive"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[test]
fn supervisor_admits_then_preserves_stdout_stderr_and_exit_code() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let probe = compile_probe(temp.path())?;
    let mut active = launch_supervisor(
        &probe,
        &["output-exit".to_string(), "23".to_string()],
        temp.path(),
    )?;

    let status = wait_bounded(&mut active.child, Duration::from_secs(5))?;
    assert_eq!(status.code(), Some(23));
    let mut stdout = String::new();
    active.stdout.read_to_string(&mut stdout)?;
    let mut stderr = String::new();
    active.stderr.read_to_string(&mut stderr)?;
    assert_eq!(stdout, "supervised-stdout");
    assert_eq!(stderr, "supervised-stderr");
    Ok(())
}

#[cfg(unix)]
#[test]
fn supervisor_propagates_target_signal() -> Result<()> {
    use std::os::unix::process::ExitStatusExt;

    let temp = tempfile::tempdir()?;
    let mut active = launch_supervisor(
        Path::new("/bin/sh"),
        &["-c".to_string(), "kill -TERM $$".to_string()],
        temp.path(),
    )?;

    let status = wait_bounded(&mut active.child, Duration::from_secs(5))?;
    assert_eq!(status.signal(), Some(libc::SIGTERM), "{status:?}");
    Ok(())
}

#[test]
fn supervisor_owner_eof_terminates_and_reaps_descendants() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let probe = compile_probe(temp.path())?;
    let pid_file = temp.path().join("descendant.pid");
    let mut active = launch_supervisor(
        &probe,
        &[
            "descendant".to_string(),
            pid_file.to_string_lossy().into_owned(),
        ],
        temp.path(),
    )?;
    let descendant = wait_for_pid(&pid_file)?;

    drop(active.stdin.take());
    let status = wait_bounded(&mut active.child, Duration::from_secs(5))?;
    assert!(!status.success());
    wait_for_process_exit(descendant)
}

#[test]
fn supervisor_root_exit_cleans_closed_pipe_descendants() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let probe = compile_probe(temp.path())?;
    let pid_file = temp.path().join("root-exit-descendant.pid");
    let mut active = launch_supervisor(
        &probe,
        &[
            "root-exit-descendant".to_string(),
            pid_file.to_string_lossy().into_owned(),
        ],
        temp.path(),
    )?;
    let descendant = wait_for_pid(&pid_file)?;

    let status = wait_bounded(&mut active.child, Duration::from_secs(5))?;
    assert_eq!(status.code(), Some(0));
    wait_for_process_exit(descendant)
}

#[test]
fn abrupt_supervisor_termination_retains_no_descendant() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let probe = compile_probe(temp.path())?;
    let pid_file = temp.path().join("abrupt-descendant.pid");
    let mut active = launch_supervisor(
        &probe,
        &[
            "descendant".to_string(),
            pid_file.to_string_lossy().into_owned(),
        ],
        temp.path(),
    )?;
    let descendant = wait_for_pid(&pid_file)?;

    active.child.kill()?;
    drop(active.stdin.take());
    let _ = wait_bounded(&mut active.child, Duration::from_secs(5))?;
    wait_for_process_exit(descendant)
}

#[cfg(unix)]
#[test]
fn abrupt_supervisor_process_group_termination_retains_no_target_tree() -> Result<()> {
    use std::os::unix::process::{CommandExt, ExitStatusExt};

    let temp = tempfile::tempdir()?;
    let probe = compile_probe(temp.path())?;
    let root_pid_file = temp.path().join("group-kill-root.pid");
    let descendant_pid_file = temp.path().join("group-kill-descendant.pid");
    let mut command = process_supervisor_command();
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut active = launch_supervisor_with_command(
        command,
        &probe,
        &[
            "descendant-with-root".to_string(),
            root_pid_file.to_string_lossy().into_owned(),
            descendant_pid_file.to_string_lossy().into_owned(),
        ],
        temp.path(),
    )?;
    let target_root = wait_for_pid(&root_pid_file)?;
    let descendant = wait_for_pid(&descendant_pid_file)?;
    let supervisor_group = active.child.id() as libc::pid_t;

    let killed = unsafe { libc::kill(-supervisor_group, libc::SIGKILL) };
    anyhow::ensure!(
        killed == 0,
        "failed killing isolated supervisor process group: {}",
        std::io::Error::last_os_error()
    );
    drop(active.stdin.take());
    let status = wait_bounded(&mut active.child, Duration::from_secs(5))?;
    assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
    wait_for_process_exit(target_root)?;
    wait_for_process_exit(descendant)
}

#[test]
fn invalid_supervisor_request_fails_before_ready() -> Result<()> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["process-supervisor", "--protocol", PROTOCOL])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(b"{}\n")?;
    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(70));
    assert_eq!(output.stdout, b"");
    let first = String::from_utf8(output.stderr)?;
    assert!(first.starts_with(CONTROL_PREFIX), "{first:?}");
    let event: serde_json::Value = serde_json::from_str(
        first
            .strip_prefix(CONTROL_PREFIX)
            .expect("control prefix")
            .trim_end(),
    )?;
    assert_eq!(event["event"], "error");
    assert_eq!(event["code"], "invalid_request");
    Ok(())
}
