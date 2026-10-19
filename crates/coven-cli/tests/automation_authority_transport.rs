#![cfg(any(unix, windows))]

//! Real-daemon transport conformance for #857 over each platform's owner IPC:
//! the Unix socket, or the owner-only Windows named pipe. Uses only an
//! isolated, paused synthetic definition: no provider credentials, live
//! harness, or user store.
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

const FIXTURE_PROMPT: &str = "Never execute this paused fixture.";

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One HTTP exchange, framed by `Content-Length` rather than end of stream:
/// a Windows pipe can report a broken pipe instead of EOF once the daemon
/// closes its end.
fn exchange(
    mut stream: impl Read + Write,
    method: &str,
    route: &str,
    body: &Value,
) -> Result<(u16, Value)> {
    let body = if method == "GET" {
        String::new()
    } else {
        body.to_string()
    };
    write!(stream, "{method} {route} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())?;
    stream.flush()?;
    let mut response = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        if let Some(end) = response.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&response[..end])?;
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .context("HTTP response has no Content-Length")?;
            let start = end + 4;
            if response.len() >= start + length {
                let status = headers
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .context("HTTP response has no status")?
                    .parse()?;
                return Ok((
                    status,
                    serde_json::from_slice(&response[start..start + length])?,
                ));
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => bail!("connection closed before a complete HTTP response"),
            Ok(read) => response.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error).context("failed reading HTTP response"),
        }
    }
}

#[cfg(unix)]
fn owner_ipc(home: &Path) -> Result<std::os::unix::net::UnixStream> {
    let stream = std::os::unix::net::UnixStream::connect(home.join("coven.sock"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(stream)
}

/// The owner-only pipe the daemon published in `daemon.json`.
#[cfg(windows)]
fn owner_ipc(home: &Path) -> Result<interprocess::local_socket::Stream> {
    use interprocess::{
        local_socket::{prelude::*, ConnectOptions, GenericNamespaced},
        ConnectWaitMode,
    };
    let status: Value = serde_json::from_slice(&std::fs::read(home.join("daemon.json"))?)?;
    let pipe = status["socket"]
        .as_str()
        .context("daemon status omitted its pipe name")?;
    Ok(ConnectOptions::new()
        .name(pipe.to_ns_name::<GenericNamespaced>()?)
        .wait_mode(ConnectWaitMode::Timeout(Duration::from_secs(2)))
        .connect_sync()?)
}

#[cfg(unix)]
fn owner_exchange(home: &Path, method: &str, route: &str, body: &Value) -> Result<(u16, Value)> {
    exchange(owner_ipc(home)?, method, route, body)
}

/// A client pipe cannot take I/O timeouts (the daemon's `set_recv_timeout`
/// applies to its accepted end only), so bound the whole exchange from outside:
/// a stalled pipe fails the test instead of wedging the job.
#[cfg(windows)]
fn owner_exchange(home: &Path, method: &str, route: &str, body: &Value) -> Result<(u16, Value)> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let (home, method, route, body) = (
        home.to_path_buf(),
        method.to_owned(),
        route.to_owned(),
        body.clone(),
    );
    std::thread::spawn(move || {
        let _ = sender
            .send(owner_ipc(&home).and_then(|stream| exchange(stream, &method, &route, &body)));
    });
    receiver
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| anyhow::anyhow!("owner pipe exchange did not finish within 10 seconds"))?
}

fn spawn_daemon(temp: &Path, home: &Path, address: SocketAddr, log: &Path) -> Result<Daemon> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
    // Windows needs SystemRoot and its other system variables, so the
    // environment is cleared only on Unix.
    #[cfg(unix)]
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    command
        .args(["daemon", "serve", "--tcp", &address.to_string()])
        .env("HOME", temp)
        .env("COVEN_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(log)?);
    Ok(Daemon(command.spawn()?))
}

#[derive(Default)]
struct ProbeObservation {
    attempts: u64,
    last_iteration: u64,
    started_us: u128,
    finished_us: u128,
    class: &'static str,
    status: Option<u16>,
    io_kind: Option<std::io::ErrorKind>,
    os_error: Option<i32>,
}

impl ProbeObservation {
    fn observe(
        &mut self,
        started: Instant,
        iteration: u64,
        probe: impl FnOnce() -> Result<(u16, Value)>,
    ) -> bool {
        self.attempts += 1;
        self.last_iteration = iteration;
        self.started_us = started.elapsed().as_micros();
        let result = probe();
        self.finished_us = started.elapsed().as_micros();
        self.status = None;
        self.io_kind = None;
        self.os_error = None;
        self.class = match result {
            Ok((status, _)) => {
                self.status = Some(status);
                if status == 200 {
                    "ready"
                } else {
                    "http-status"
                }
            }
            Err(error) => {
                if let Some(error) = error.downcast_ref::<std::io::Error>() {
                    self.io_kind = Some(error.kind());
                    self.os_error = error.raw_os_error();
                    "io-error"
                } else if error.downcast_ref::<serde_json::Error>().is_some() {
                    "json-error"
                } else {
                    "other-error"
                }
            }
        };
        self.status == Some(200)
    }
}

impl std::fmt::Display for ProbeObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "attempts={} last_iteration={} started_us={} finished_us={} class={} status={:?} io_kind={:?} os_error={:?}",
            self.attempts, self.last_iteration, self.started_us, self.finished_us,
            self.class, self.status, self.io_kind, self.os_error)
    }
}

fn wait_for_ready(
    child: &mut Child,
    started: Instant,
    deadline: Instant,
    log_path: &Path,
    mut ipc: impl FnMut() -> Result<(u16, Value)>,
    mut tcp: impl FnMut() -> Result<(u16, Value)>,
    tcp_required: bool,
) -> Result<()> {
    let mut ipc_observation = ProbeObservation {
        class: "not-attempted",
        ..Default::default()
    };
    let mut tcp_observation = ProbeObservation {
        class: "not-attempted",
        ..Default::default()
    };
    let mut iteration = 0;
    loop {
        iteration += 1;
        let ipc_ready = ipc_observation.observe(started, iteration, &mut ipc);
        let tcp_attempted = ipc_ready && tcp_required;
        if ipc_ready && (!tcp_required || tcp_observation.observe(started, iteration, &mut tcp)) {
            return Ok(());
        }
        // Preserve the original single poll and IPC-first short circuit. Record
        // only static classes/numeric status; never format probe error chains,
        // response bodies, addresses or paths into the new projection.
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => bail!(
                "isolated daemon readiness poll failed: elapsed_us={} io_kind={:?} os_error={:?} iteration={} ipc=[{}] tcp_attempted_this_iteration={} tcp=[{}]",
                started.elapsed().as_micros(), error.kind(), error.raw_os_error(),
                iteration, ipc_observation, tcp_attempted, tcp_observation
            ),
        };
        let deadline_expired = Instant::now() >= deadline;
        if status.is_some() || deadline_expired {
            let child_state = if let Some(status) = status {
                #[cfg(unix)]
                let signal = {
                    use std::os::unix::process::ExitStatusExt;
                    status.signal()
                };
                #[cfg(windows)]
                let signal: Option<i32> = None;
                format!(
                    "Exited {{ code: {:?}, signal: {:?} }}",
                    status.code(),
                    signal
                )
            } else {
                "Running".to_owned()
            };
            // Preserve valid daemon stderr, but never let an attachment failure
            // replace the primary readiness observations or expose its path.
            let attachment = std::fs::read_to_string(log_path).unwrap_or_else(|error| {
                format!(
                    "[log_attachment=unavailable io_kind={:?} os_error={:?}]",
                    error.kind(),
                    error.raw_os_error()
                )
            });
            bail!(
                "isolated daemon did not become ready: elapsed_us={} deadline_expired={} child={} iteration={} ipc=[{}] tcp_attempted_this_iteration={} tcp=[{}]: {}",
                started.elapsed().as_micros(), deadline_expired, child_state,
                iteration, ipc_observation, tcp_attempted, tcp_observation,
                attachment
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

// These owned shell fixtures exercise the readiness failure seam, not daemon
// startup. Closing/killing and waiting remain covered by Daemon on every path.
fn readiness_fixture(live: bool) -> Result<Daemon> {
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", if live { "read fixture" } else { "exit 23" }]);
        command
    };
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/C", if live { "set /p fixture=" } else { "exit 23" }]);
        command
    };
    Ok(Daemon(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    ))
}

#[test]
fn readiness_failure_retains_exited_child_status_with_empty_stderr() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let log = temp.path().join("empty.log");
    std::fs::write(&log, "")?;
    let mut child = readiness_fixture(false)?;
    // Only an orphan/hang guard: normal exit is announced by try_wait, not a
    // sleep or a tight promptness assumption on the shared runner.
    let started = Instant::now();
    let guard = started + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.0.try_wait()? {
            break status;
        }
        anyhow::ensure!(
            Instant::now() < guard,
            "exit fixture did not finish: elapsed={:?}",
            started.elapsed()
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(status.code(), Some(23));
    let failure = wait_for_ready(
        &mut child.0,
        Instant::now(),
        Instant::now() + Duration::from_secs(30),
        &log,
        || Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into()),
        || panic!("TCP must remain uncalled after failed IPC"),
        cfg!(unix),
    )
    .unwrap_err()
    .to_string();
    assert!(failure.contains("child=Exited"), "{failure}");
    assert!(failure.contains("code: Some(23)"), "{failure}");
    Ok(())
}

#[test]
fn readiness_failure_distinguishes_live_child_at_expired_fixture_deadline() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let log = temp.path().join("empty.log");
    std::fs::write(&log, "")?;
    let mut child = readiness_fixture(true)?;
    // This deliberately expired classification fixture does not alter the real
    // daemon's 30s setup budget. The owned fixture is intended to wait on stdin;
    // Running below reports only the actual pre-cleanup child poll.
    let failure = wait_for_ready(
        &mut child.0,
        Instant::now(),
        Instant::now(),
        &log,
        || anyhow::bail!("private-probe-sentinel"),
        || panic!("TCP must remain uncalled after failed IPC"),
        cfg!(unix),
    )
    .unwrap_err()
    .to_string();
    assert!(failure.contains("child=Running"), "{failure}");
    assert!(failure.contains("deadline_expired=true"), "{failure}");
    assert!(!failure.contains("private-probe-sentinel"), "{failure}");
    Ok(())
}

#[test]
fn readiness_failure_reports_tcp_only_after_successful_ipc_without_private_values() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let log = temp.path().join("empty.log");
    std::fs::write(&log, "")?;
    let mut child = readiness_fixture(true)?;
    let failure = wait_for_ready(
        &mut child.0,
        Instant::now(),
        Instant::now(),
        &log,
        || Ok((200, json!({"private": "private-body-sentinel"}))),
        || {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "private-error-sentinel",
            )
            .into())
        },
        true,
    )
    .unwrap_err()
    .to_string();
    assert!(failure.contains("ipc=[attempts=1"), "{failure}");
    assert!(
        failure.contains("tcp_attempted_this_iteration=true"),
        "{failure}"
    );
    assert!(failure.contains("class=io-error"), "{failure}");
    assert!(failure.contains("PermissionDenied"), "{failure}");
    assert!(!failure.contains("private-body-sentinel"), "{failure}");
    assert!(!failure.contains("private-error-sentinel"), "{failure}");
    Ok(())
}

#[test]
fn readiness_failure_survives_invalid_utf8_log_attachment() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let log = temp.path().join("private-log-path-sentinel");
    std::fs::write(&log, [0xff])?;
    let mut child = readiness_fixture(true)?;
    let failure = wait_for_ready(
        &mut child.0,
        Instant::now(),
        Instant::now(),
        &log,
        || anyhow::bail!("private-probe-sentinel"),
        || panic!("TCP must remain uncalled after failed IPC"),
        cfg!(unix),
    )
    .unwrap_err()
    .to_string();
    assert!(failure.contains("child=Running"), "{failure}");
    assert!(failure.contains("deadline_expired=true"), "{failure}");
    assert!(failure.contains("log_attachment=unavailable"), "{failure}");
    assert!(failure.contains("io_kind=InvalidData"), "{failure}");
    assert!(!failure.contains("private-log-path-sentinel"), "{failure}");
    assert!(!failure.contains("private-probe-sentinel"), "{failure}");
    Ok(())
}

#[test]
fn readiness_failure_preserves_valid_stderr_attachment() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let log = temp.path().join("fixture.log");
    let stderr = "public fixture daemon stderr\n";
    std::fs::write(&log, stderr)?;
    let mut child = readiness_fixture(true)?;
    let failure = wait_for_ready(
        &mut child.0,
        Instant::now(),
        Instant::now(),
        &log,
        || anyhow::bail!("private-probe-sentinel"),
        || panic!("TCP must remain uncalled after failed IPC"),
        cfg!(unix),
    )
    .unwrap_err()
    .to_string();
    assert!(failure.contains("child=Running"), "{failure}");
    assert!(failure.ends_with(stderr), "{failure}");
    assert!(!failure.contains("private-probe-sentinel"), "{failure}");
    Ok(())
}

#[test]
fn automation_mutations_require_real_owner_ipc_not_loopback_tcp() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("coven-home");
    let reservation = TcpListener::bind("127.0.0.1:0")?;
    let address = reservation.local_addr()?;
    drop(reservation);
    let log_path = temp.path().join("daemon.log");
    let mut daemon = spawn_daemon(temp.path(), &home, address, &log_path)?;
    let tcp = |method: &str, route: &str, body: &Value| -> Result<(u16, Value)> {
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        exchange(stream, method, route, body)
    };
    let ipc = |method: &str, route: &str, body: &Value| -> Result<(u16, Value)> {
        owner_exchange(&home, method, route, body)
    };
    let started = Instant::now();
    let deadline = started + Duration::from_secs(30);
    wait_for_ready(
        &mut daemon.0,
        started,
        deadline,
        &log_path,
        || ipc("GET", "/health", &Value::Null),
        || tcp("GET", "/health", &Value::Null),
        cfg!(unix),
    )?;
    // Unix serves `--tcp`. The Windows daemon ignores it and binds no TCP
    // listener, so its owner-only pipe is the whole surface, and anything on the
    // released port there belongs to another process. Enable this for Windows
    // when its daemon starts serving TCP.
    let tcp_served = cfg!(unix);
    let create = json!({
        "action": "coven.automations.definition.create.v1",
        "adoptionKey": "wire-owner-adoption",
        "origin": "owner-local-ipc", "principalId": "owner", "authority": "OwnerLocalIpc",
        "definition": {
            "schemaVersion": 1, "id": "wire-authority-fixture", "name": "Wire fixture",
            "status": "PAUSED", "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc",
            "misfire": "latest", "overlap": "forbid", "timeoutMinutes": 30,
            "runtime": "coven-code", "prompt": FIXTURE_PROMPT
        }
    });
    if tcp_served {
        for action in [
            "definition.create.v1",
            "definition.revise.v1",
            "definition.disable.v1",
            "definition.tombstone.v1",
            "definition.activate.v1",
            "definition.pause.v1",
            "create",
            "update",
            "delete",
            "tick",
            "run",
            "import",
            "run.cancel.v1",
            "receipt.get.v1",
            "futureMutation.v2",
        ] {
            for route in ["/actions", "/api/v1/actions"] {
                let mut body = create.clone();
                body["action"] = json!(format!("coven.automations.{action}"));
                let (status, response) = tcp("POST", route, &body)?;
                assert_eq!(status, 403, "{action}: {response}");
                assert_eq!(response["error"]["code"], "AUTHORITY_REQUIRED");
                assert_eq!(response["accepted"], false);
            }
        }
        let conn = rusqlite::Connection::open_with_flags(
            home.join("coven.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        for table in [
            "automation_definitions",
            "automation_command_adoptions",
            "automation_command_reservations",
        ] {
            let count: i64 =
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            assert_eq!(count, 0, "TCP refusal wrote {table}");
        }
    }

    // The real owner transport is classified as owner-local: mutations commit
    // and replay through it.
    let (status, owner) = ipc("POST", "/api/v1/actions", &create)?;
    assert_eq!(status, 200, "{owner}");
    assert_eq!(owner["result"]["outcome"], "committed");
    if tcp_served {
        let (status, refused_replay) = tcp("POST", "/api/v1/actions", &create)?;
        assert_eq!(status, 403, "{refused_replay}");
    }
    let (status, owner_replay) = ipc("POST", "/api/v1/actions", &create)?;
    assert_eq!(status, 200, "{owner_replay}");
    assert_eq!(owner_replay["result"]["outcome"], "replayed");

    // Owner-only reads (prompts, run logs, events) are served over the owner
    // transport and refused over TCP; scheduling diagnostics stay on TCP.
    let (status, owner_read) = ipc(
        "POST",
        "/api/v1/actions",
        &json!({"action": "coven.automations.definition.get.v1", "id": "wire-authority-fixture"}),
    )?;
    assert_eq!(status, 200, "{owner_read}");
    assert!(
        owner_read.to_string().contains(FIXTURE_PROMPT),
        "{owner_read}"
    );
    let (status, owner_events) = ipc(
        "POST",
        "/api/v1/actions",
        &json!({
            "action": "coven.automations.events.read.v1",
            "stream": {"kind": "automation", "id": "wire-authority-fixture"},
        }),
    )?;
    assert_eq!(status, 200, "{owner_events}");
    if tcp_served {
        let (status, refused_read) = tcp(
            "POST",
            "/api/v1/actions",
            &json!({"action": "coven.automations.definition.list.v1"}),
        )?;
        assert_eq!(status, 403, "{refused_read}");
        assert!(!refused_read.to_string().contains(FIXTURE_PROMPT));
        let (status, read) = tcp(
            "POST",
            "/api/v1/actions",
            &json!({"action": "coven.automations.scheduler.status.v1"}),
        )?;
        assert_eq!(status, 200, "read-only compatibility: {read}");
    }
    Ok(())
}
