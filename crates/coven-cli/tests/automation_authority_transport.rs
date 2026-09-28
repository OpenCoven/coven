#![cfg(unix)]

//! Real-daemon transport conformance for #857. Uses only an isolated, paused
//! synthetic definition: no provider credentials, live harness, or user store.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

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
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .context("HTTP response has no body separator")?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .context("HTTP response has no status")?
        .parse()?;
    Ok((status, serde_json::from_str(body)?))
}

#[test]
fn automation_mutations_require_real_owner_ipc_not_loopback_tcp() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("coven-home");
    let reservation = TcpListener::bind("127.0.0.1:0")?;
    let address = reservation.local_addr()?;
    drop(reservation);
    let log_path = temp.path().join("daemon.log");
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_coven"))
            .args(["daemon", "serve", "--tcp", &address.to_string()])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", temp.path())
            .env("COVEN_HOME", &home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log_path)?)
            .spawn()?,
    );
    let tcp = |method: &str, route: &str, body: &Value| -> Result<(u16, Value)> {
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        exchange(stream, method, route, body)
    };
    let ipc = |method: &str, route: &str, body: &Value| -> Result<(u16, Value)> {
        let stream = UnixStream::connect(home.join("coven.sock"))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        exchange(stream, method, route, body)
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if matches!(ipc("GET", "/health", &Value::Null), Ok((200, _)))
            && matches!(tcp("GET", "/health", &Value::Null), Ok((200, _)))
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
    let create = json!({
        "action": "coven.automations.definition.create.v1",
        "adoptionKey": "wire-owner-adoption",
        "origin": "owner-local-ipc", "principalId": "owner", "authority": "OwnerLocalIpc",
        "definition": {
            "schemaVersion": 1, "id": "wire-authority-fixture", "name": "Wire fixture",
            "status": "PAUSED", "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc",
            "misfire": "latest", "overlap": "forbid", "timeoutMinutes": 30,
            "runtime": "coven-code", "prompt": "Never execute this paused fixture."
        }
    });
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
        let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 0, "TCP refusal wrote {table}");
    }
    let (status, owner) = ipc("POST", "/api/v1/actions", &create)?;
    assert_eq!(status, 200, "{owner}");
    assert_eq!(owner["result"]["outcome"], "committed");
    let (status, refused_replay) = tcp("POST", "/api/v1/actions", &create)?;
    assert_eq!(status, 403, "{refused_replay}");
    let (status, owner_replay) = ipc("POST", "/api/v1/actions", &create)?;
    assert_eq!(status, 200, "{owner_replay}");
    assert_eq!(owner_replay["result"]["outcome"], "replayed");
    let (status, read) = tcp(
        "POST",
        "/api/v1/actions",
        &json!({"action": "coven.automations.definition.list.v1"}),
    )?;
    assert_eq!(status, 200, "read-only compatibility: {read}");
    Ok(())
}
