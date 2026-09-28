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
        exchange(owner_ipc(&home)?, method, route, body)
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if matches!(ipc("GET", "/health", &Value::Null), Ok((200, _)))
            && (cfg!(windows) || matches!(tcp("GET", "/health", &Value::Null), Ok((200, _))))
        {
            break;
        }
        if daemon.0.try_wait()?.is_some() || Instant::now() >= deadline {
            bail!(
                "isolated daemon did not become ready: {}",
                std::fs::read_to_string(&log_path)?
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    // Unix serves `--tcp`; the Windows daemon does not bind TCP at all, so its
    // owner-only pipe is the whole surface. Should Windows gain a TCP listener,
    // every TCP refusal below applies to it unchanged.
    let tcp_served =
        cfg!(unix) || TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok();
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
