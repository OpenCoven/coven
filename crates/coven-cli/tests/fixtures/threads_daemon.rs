//! Small lifecycle/IPC fixture extracted from PR #931's threads_e2e.rs
//! (8576f41e6d622f63a3576b85bd2d3142776e59ca, Val Alexander).
//! No source includes, dependency overrides, clock controls, or policy generators.

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

pub const FAMILIAR_ID: &str = "sage";
pub const PRINCIPAL_FINGERPRINT: &str = "fpr-e2e-synthetic";
// Hang guard, not a promptness claim; allow shared CI scheduler jitter.
const LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(15);

pub fn run_journey(journey: impl FnOnce(&mut ThreadsFixture) -> Result<()>) -> Result<()> {
    let mut fixture = ThreadsFixture::start()?;
    let result = journey(&mut fixture);
    match (result, fixture.stop_daemon()) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error.context("journey passed but daemon shutdown failed")),
        (Err(error), Err(shutdown)) => {
            Err(error.context(format!("daemon shutdown also failed: {shutdown:#}")))
        }
    }
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Value,
}

pub struct ThreadsFixture {
    _temp: tempfile::TempDir,
    pub coven_home: PathBuf,
    pub workspace: PathBuf,
    daemon_pid: Option<u32>,
    stopped: bool,
}

impl ThreadsFixture {
    fn start() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let coven_home = temp.path().join("coven-home");
        let workspace = coven_home.join("familiars").join(FAMILIAR_ID);
        fs::create_dir_all(&workspace)?;
        fs::write(
            coven_home.join("familiars.toml"),
            r#"[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "Synthetic protected-intake familiar."
"#,
        )?;
        fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
        fs::write(
            workspace.join("ward.toml"),
            format!(
                r#"principal_key_fingerprint = "{PRINCIPAL_FINGERPRINT}"
protected_surface = ["SOUL.md"]

[[surface]]
path = "SOUL.md"
tier = 0

[[surface]]
path = "reviewed/"
tier = 1

[[probe]]
surface = "reviewed/**"
id = "size-delta"

[[probe]]
surface = "reviewed/**"
id = "pattern-lint"
forbidden = ["(?i)ignore previous"]
"#
            ),
        )?;
        let mut fixture = Self {
            _temp: temp,
            coven_home,
            workspace,
            daemon_pid: None,
            stopped: true,
        };
        fixture.start_daemon()?;
        Ok(fixture)
    }

    pub fn start_daemon(&mut self) -> Result<()> {
        anyhow::ensure!(self.stopped, "fixture daemon must be stopped before start");
        self.stopped = false;
        self.daemon_command("start")?;
        self.wait_for_health()?;
        self.daemon_pid = Some(self.read_daemon_pid()?);
        Ok(())
    }

    pub fn request(
        &self,
        method: &'static str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse> {
        // The pinned client has no generic Threads request method. Keep the
        // original fixture's HTTP wire approach, not PR #931's client changes.
        coven_client::DaemonEndpoint::discover(&self.coven_home)?;
        let mut stream = self.connect()?;
        let body = body
            .map(serde_json::to_string)
            .transpose()?
            .unwrap_or_default();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        exchange_http(&mut stream, request.as_bytes())
            .with_context(|| format!("fixture daemon request {method} {path}"))
    }

    fn connect(&self) -> Result<impl Read + Write> {
        #[cfg(unix)]
        let stream = std::os::unix::net::UnixStream::connect(self.coven_home.join("coven.sock"))?;
        #[cfg(windows)]
        let stream = {
            use interprocess::{
                local_socket::{prelude::*, ConnectOptions, GenericNamespaced},
                ConnectWaitMode,
            };
            let status: Value =
                serde_json::from_slice(&fs::read(self.coven_home.join("daemon.json"))?)?;
            let pipe = status["socket"]
                .as_str()
                .context("daemon status missing pipe")?;
            ConnectOptions::new()
                .name(pipe.to_ns_name::<GenericNamespaced>()?)
                .wait_mode(ConnectWaitMode::Timeout(LIFECYCLE_TIMEOUT))
                .connect_sync_as::<WindowsPipe>()?
        };
        #[cfg(windows)]
        use interprocess::local_socket::prelude::*;
        stream.set_nonblocking(true)?;
        #[cfg(unix)]
        {
            Ok(stream)
        }
        #[cfg(windows)]
        {
            Ok(NonblockingPipe {
                stream,
                probe: windows_pipe_connected,
            })
        }
    }

    pub fn store(&self) -> Result<Connection> {
        Connection::open_with_flags(
            self.coven_home.join("coven.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(Into::into)
    }

    fn daemon_command(&self, operation: &str) -> Result<Output> {
        let started = Instant::now();
        let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
        command.env_clear();
        // Keep only OS launch necessities, not developer credentials or test controls.
        for key in ["PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let output = command
            .args(["daemon", operation])
            .current_dir(&self.workspace)
            .env("COVEN_HOME", &self.coven_home)
            .env("HOME", self._temp.path())
            .env("USERPROFILE", self._temp.path())
            .env("XDG_CONFIG_HOME", self._temp.path().join("config"))
            .env("XDG_DATA_HOME", self._temp.path().join("data"))
            .env("XDG_CACHE_HOME", self._temp.path().join("cache"))
            .env("TMPDIR", self._temp.path())
            .env("TEMP", self._temp.path())
            .env("TMP", self._temp.path())
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "daemon {operation} failed after {:?}\nstdout:\n{}\nstderr:\n{}\n\
             fixture status: {:?}\nfixture recovery log: {:?}",
            started.elapsed(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            fs::read_to_string(self.coven_home.join("daemon.json")),
            fs::read_to_string(self.coven_home.join("daemon-recovery.log")),
        );
        Ok(output)
    }

    fn read_daemon_pid(&self) -> Result<u32> {
        let status: Value =
            serde_json::from_slice(&fs::read(self.coven_home.join("daemon.json"))?)?;
        let pid = status["pid"]
            .as_u64()
            .context("daemon status missing pid")?;
        u32::try_from(pid).context("daemon pid does not fit u32")
    }

    fn current_daemon_pid(&self) -> Option<u32> {
        self.daemon_pid
            .filter(|pid| pid_is_alive(*pid))
            .or_else(|| self.read_daemon_pid().ok().filter(|pid| pid_is_alive(*pid)))
    }

    fn wait_for_health(&self) -> Result<()> {
        let started = Instant::now();
        loop {
            let response = self.request("GET", "/health", None);
            if response
                .as_ref()
                .is_ok_and(|r| r.status == 200 && r.body["ok"] == true)
            {
                return Ok(());
            }
            anyhow::ensure!(
                started.elapsed() < LIFECYCLE_TIMEOUT,
                "daemon did not become healthy after {:?}: {response:?}",
                started.elapsed()
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn stop_daemon(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        let pid = self.current_daemon_pid();
        self.daemon_command("stop")?;
        let started = Instant::now();
        while pid.is_some_and(pid_is_alive)
            || self.coven_home.join("daemon.json").exists()
            || self.coven_home.join("coven.sock").exists()
        {
            anyhow::ensure!(
                started.elapsed() < LIFECYCLE_TIMEOUT,
                "daemon {pid:?} or its status/socket remained after stop for {:?}",
                started.elapsed()
            );
            thread::sleep(Duration::from_millis(25));
        }
        self.daemon_pid = None;
        self.stopped = true;
        Ok(())
    }

    pub fn restart_daemon(&mut self) -> Result<()> {
        let before = self
            .current_daemon_pid()
            .context("restart requires a live daemon")?;
        self.daemon_command("restart")?;
        self.wait_for_health()?;
        let after = self.read_daemon_pid()?;
        self.daemon_pid = Some(after);
        anyhow::ensure!(
            before != after,
            "restart did not replace daemon pid {before}"
        );
        anyhow::ensure!(
            !pid_is_alive(before),
            "restart left old daemon pid {before} alive"
        );
        Ok(())
    }
}

impl Drop for ThreadsFixture {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        let pid = self.current_daemon_pid();
        if let Err(error) = self.stop_daemon() {
            eprintln!("threads fixture graceful shutdown failed: {error:#}");
        }
        if let Some(pid) = pid.filter(|pid| pid_is_alive(*pid)) {
            eprintln!("threads fixture fallback terminating its daemon pid {pid}");
            if let Err(error) = self.terminate_daemon_process(pid) {
                eprintln!("failed terminating fixture daemon pid {pid}: {error:#}");
            }
        }
    }
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        System::Threading::{OpenProcess, WaitForSingleObject},
    };
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    let handle = unsafe { OpenProcess(SYNCHRONIZE_ACCESS, 0, pid) };
    if handle.is_null() {
        let error = unsafe { GetLastError() };
        assert_eq!(
            error, ERROR_INVALID_PARAMETER,
            "cannot probe daemon pid {pid}"
        );
        return false;
    }
    let result = unsafe { WaitForSingleObject(handle, 0) };
    unsafe { CloseHandle(handle) };
    match result {
        WAIT_TIMEOUT => true,
        WAIT_OBJECT_0 => false,
        _ => panic!("cannot probe daemon pid {pid}: wait result {result}"),
    }
}

impl ThreadsFixture {
    fn terminate_daemon_process(&self, pid: u32) -> Result<()> {
        #[cfg(unix)]
        {
            let status = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status()?;
            anyhow::ensure!(
                status.success(),
                "SIGKILL failed for fixture daemon pid {pid}"
            );
        }
        #[cfg(windows)]
        {
            let status: Value =
                serde_json::from_slice(&fs::read(self.coven_home.join("daemon.json"))?)?;
            let pipe = status["socket"]
                .as_str()
                .context("daemon status missing pipe")?;
            let deadline = Instant::now() + LIFECYCLE_TIMEOUT;
            let probe =
                coven_client::probe_windows_daemon_health_with_identity_until(pipe, deadline)?
                    .context("fixture pipe disappeared before fallback cleanup")?;
            anyhow::ensure!(probe.server_pid == pid, "fixture daemon identity changed");
            let process = coven_client::open_windows_daemon_process_for_stop_until(
                pipe,
                pid,
                Some(probe.process_creation_time),
                deadline,
            )?
            .context("fixture daemon identity could not be verified")?;
            anyhow::ensure!(
                process.terminate_and_wait_until(deadline)?,
                "daemon pid {pid} did not exit"
            );
        }
        let started = Instant::now();
        while pid_is_alive(pid) {
            anyhow::ensure!(
                started.elapsed() < LIFECYCLE_TIMEOUT,
                "terminated fixture daemon pid {pid} remained observable for {:?}",
                started.elapsed()
            );
            thread::sleep(Duration::from_millis(25));
        }
        Ok(())
    }
}

fn exchange_http(stream: &mut (impl Read + Write), mut request: &[u8]) -> Result<HttpResponse> {
    let started = Instant::now();
    let mut response = Vec::new();
    let mut frame = None;
    loop {
        anyhow::ensure!(
            started.elapsed() < LIFECYCLE_TIMEOUT,
            "fixture HTTP exchange timed out after {:?}",
            started.elapsed()
        );
        if !request.is_empty() {
            match stream.write(request) {
                Ok(0) => anyhow::bail!("daemon closed while writing fixture request"),
                Ok(written) => request = &request[written..],
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
            continue;
        }
        let mut buffer = [0_u8; 8192];
        match stream.read(&mut buffer) {
            Ok(0) => anyhow::bail!("daemon closed before a complete HTTP response"),
            Ok(read) => response.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
        anyhow::ensure!(
            response.len() <= 1024 * 1024,
            "fixture response exceeded 1 MiB"
        );
        if frame.is_none() {
            if let Some(header_end) = response.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&response[..header_end])?;
                let status = headers
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .context("daemon response missing HTTP status")?
                    .parse::<u16>()?;
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>())
                    })
                    .transpose()?
                    .context("daemon response missing Content-Length")?;
                anyhow::ensure!(
                    length <= 1024 * 1024,
                    "fixture response body exceeded 1 MiB"
                );
                frame = Some((status, header_end + 4, length));
            }
        }
        if let Some((status, body_start, length)) = frame {
            if response.len() >= body_start + length {
                return Ok(HttpResponse {
                    status,
                    body: serde_json::from_slice(&response[body_start..body_start + length])
                        .context("daemon returned non-JSON response")?,
                });
            }
        }
    }
}

#[cfg(any(windows, test))]
struct NonblockingPipe<S, P> {
    stream: S,
    probe: P,
}

#[cfg(any(windows, test))]
impl<S: Read, P: FnMut(&S) -> io::Result<()>> Read for NonblockingPipe<S, P> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        match self.stream.read(buffer) {
            // interprocess 2.4.4 downgrades ERROR_NO_DATA and real pipe
            // disconnects alike to Ok(0). Retry only a still-connected pipe.
            Ok(0) => {
                (self.probe)(&self.stream)?;
                Err(io::ErrorKind::WouldBlock.into())
            }
            result => result,
        }
    }
}

#[cfg(any(windows, test))]
impl<S: Write, P: FnMut(&S) -> io::Result<()>> Write for NonblockingPipe<S, P> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        match self.stream.write(buffer) {
            // A connected PIPE_NOWAIT writer can report zero under backpressure.
            Ok(0) => {
                (self.probe)(&self.stream)?;
                Err(io::ErrorKind::WouldBlock.into())
            }
            result => result,
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

#[cfg(windows)]
type WindowsPipe = interprocess::os::windows::named_pipe::local_socket::Stream;

#[cfg(windows)]
fn windows_pipe_connected(stream: &WindowsPipe) -> io::Result<()> {
    use std::os::windows::io::{AsHandle, AsRawHandle};
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    // SAFETY: the stream owns this connected pipe handle; null buffer/output
    // pointers request only a non-consuming connection probe.
    let connected = unsafe {
        PeekNamedPipe(
            stream.as_handle().as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if connected == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use std::collections::VecDeque;

    struct ScriptedStream(VecDeque<&'static [u8]>);

    impl Read for ScriptedStream {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let chunk = self.0.pop_front().expect("unexpected extra fixture read");
            assert!(chunk.len() <= buffer.len());
            buffer[..chunk.len()].copy_from_slice(chunk);
            Ok(chunk.len())
        }
    }

    impl Write for ScriptedStream {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn idle_pipe_reads_before_and_during_response_are_retried() -> Result<()> {
        let mut pipe = NonblockingPipe {
            stream: ScriptedStream(VecDeque::from([
                b"".as_slice(),
                b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n",
                b"",
                br#"{"ok":"#,
                b"",
                b"true}",
            ])),
            probe: |_: &ScriptedStream| Ok(()),
        };
        let response = exchange_http(&mut pipe, b"GET /health HTTP/1.1\r\n\r\n")?;
        assert_eq!(response.status, 200);
        assert_eq!(response.body, serde_json::json!({"ok": true}));
        assert!(pipe.stream.0.is_empty());
        Ok(())
    }

    #[test]
    fn disconnected_pipe_preserves_the_probe_error() {
        let mut pipe = NonblockingPipe {
            stream: ScriptedStream(VecDeque::from([b"".as_slice()])),
            probe: |_: &ScriptedStream| Err(io::Error::from_raw_os_error(109)),
        };
        let error = exchange_http(&mut pipe, b"request").unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<io::Error>()
                .and_then(io::Error::raw_os_error),
            Some(109),
            "{error:#}"
        );
    }

    #[test]
    fn socket_eof_still_rejects_an_incomplete_response() {
        let mut socket = ScriptedStream(VecDeque::from([b"".as_slice()]));
        let error = exchange_http(&mut socket, b"request").unwrap_err();
        assert!(error
            .to_string()
            .contains("closed before a complete HTTP response"));
    }

    #[test]
    fn empty_pipe_read_buffer_does_not_probe_or_consume_data() {
        let mut pipe = NonblockingPipe {
            stream: ScriptedStream(VecDeque::new()),
            probe: |_: &ScriptedStream| panic!("empty read must not probe the pipe"),
        };
        assert_eq!(pipe.read(&mut []).unwrap(), 0);
    }

    #[test]
    fn zero_pipe_write_retries_only_while_connected() {
        let mut buffer = [0_u8; 1];
        let mut pipe = NonblockingPipe {
            stream: io::Cursor::new(buffer.as_mut_slice()),
            probe: |_: &io::Cursor<&mut [u8]>| -> io::Result<()> { Ok(()) },
        };
        assert_eq!(pipe.write(b"ab").unwrap(), 1);
        assert_eq!(
            pipe.write(b"b").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        pipe.stream.set_position(0);
        assert_eq!(pipe.write(b"b").unwrap(), 1);
    }

    #[test]
    fn zero_pipe_write_preserves_disconnect_error() {
        let mut buffer = [];
        let mut pipe = NonblockingPipe {
            stream: io::Cursor::new(buffer.as_mut_slice()),
            probe: |_: &io::Cursor<&mut [u8]>| -> io::Result<()> {
                Err(io::Error::from_raw_os_error(109))
            },
        };
        assert_eq!(
            pipe.write(b"request").unwrap_err().raw_os_error(),
            Some(109)
        );
    }

    #[test]
    fn empty_pipe_write_does_not_probe() {
        let mut pipe = NonblockingPipe {
            stream: io::Cursor::new(Vec::new()),
            probe: |_: &io::Cursor<Vec<u8>>| panic!("empty write must not probe the pipe"),
        };
        assert_eq!(pipe.write(&[]).unwrap(), 0);
    }

    #[test]
    fn permanently_idle_pipe_still_expires_at_the_http_deadline() {
        let mut pipe = NonblockingPipe {
            stream: io::Cursor::new(Vec::new()),
            probe: |_: &io::Cursor<Vec<u8>>| Ok(()),
        };
        let error = exchange_http(&mut pipe, b"request").unwrap_err();
        assert!(error
            .to_string()
            .contains("fixture HTTP exchange timed out"));
    }
}
