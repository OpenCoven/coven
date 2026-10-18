#![cfg(any(unix, windows))]
//! Disposable-daemon lease reads. Synthetic terminal remnants cannot dispatch.
use anyhow::{bail, Context, Result};
use coven_client::{DaemonClient, DaemonEndpoint};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
#[cfg(unix)]
use std::io::{Read, Write};
use std::net::TcpListener;
#[cfg(unix)]
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const ACTION: &str = "coven.automations.occurrence.leases.v1";
struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn cli(root: &Path, home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coven"));
    #[cfg(unix)]
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    command
        .env_remove("COVEN_MAINTENANCE_PARTICIPANT")
        .env("HOME", root)
        .env("COVEN_HOME", home)
        .stdin(Stdio::null());
    command
}
fn request(home: &Path, payload: &Value) -> Result<Value> {
    let response = DaemonClient::new(DaemonEndpoint::discover(home)?).request_json(
        "POST",
        "/api/v1/actions",
        Some(payload),
    )?;
    assert_eq!(response.status, 200);
    let response: Value = serde_json::from_slice(&response.body)?;
    assert_eq!(response["ok"], true);
    assert!(response["event"].is_null());
    Ok(response["result"].clone())
}
fn snapshot(conn: &Connection) -> Result<Vec<Vec<String>>> {
    let mut all = Vec::new();
    for table in [
        "automation_occurrences",
        "automation_runs",
        "automation_attempts",
        "automation_command_adoptions",
        "automation_command_reservations",
        "automation_events",
    ] {
        let mut statement = conn.prepare(&format!("SELECT * FROM {table} ORDER BY 1"))?;
        let columns = statement.column_count();
        all.push(
            statement
                .query_map([], |row| {
                    let mut values = Vec::new();
                    for column in 0..columns {
                        values.push(format!("{:?}", row.get_ref(column)?));
                    }
                    Ok(values.join("|"))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Ok(all)
}

#[test]
fn real_daemon_lease_pages_are_minimized_read_only_and_owner_local() -> Result<()> {
    let root = tempfile::tempdir()?;
    let home = root.path().join("home");
    let reservation = TcpListener::bind("127.0.0.1:0")?;
    let address = reservation.local_addr()?;
    drop(reservation);
    let mut daemon = Daemon(
        cli(root.path(), &home)
            .args(["daemon", "serve", "--tcp", &address.to_string()])
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(root.path().join("daemon.log"))?)
            .spawn()?,
    );
    // Generous hang guard; readiness is established by authenticated health.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(endpoint) = DaemonEndpoint::discover(&home) {
            if DaemonClient::new(endpoint).health().is_ok() {
                break;
            }
        }
        if daemon.0.try_wait()?.is_some() || Instant::now() >= deadline {
            bail!("isolated lease daemon did not become ready");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let mut conn = Connection::open(home.join("coven.sqlite3"))?;
    conn.busy_timeout(Duration::from_secs(10))?;
    let transaction = conn.transaction()?;
    for index in 0..112 {
        for (prefix, automation, state, owner, expiry) in [
            ("000-unleased", "target", "planned", None, None),
            (
                "001-other",
                "other",
                "failed",
                Some("private-owner-marker"),
                Some("2000-01-01T00:00:00Z"),
            ),
            (
                "lease",
                "target",
                "failed",
                Some("private-owner-marker"),
                Some(if index % 2 == 0 {
                    "2000-01-01T00:00:00Z"
                } else {
                    "private-invalid-expiry"
                }),
            ),
        ] {
            let id = format!("{prefix}-{index:03}");
            transaction.execute("INSERT INTO automation_occurrences (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at, scheduler_generation, attempt, failure_reason, created_at, updated_at) VALUES (?1, ?2, ?1, ?3, ?4, ?5, 7, 2, 'private-reason-marker', ?1, ?1)", params![id, automation, state, owner, expiry])?;
        }
    }
    transaction.commit()?;
    let before = snapshot(&conn)?;
    let first = request(
        &home,
        &json!({"action":ACTION,"automationId":"target","limit":100}),
    )?;
    assert_eq!(first["schemaVersion"], "coven.automations.leases.v1");
    assert_eq!(first["leases"].as_array().context("leases")?.len(), 100);
    assert_eq!(first["leases"][0]["occurrenceId"], "lease-000");
    assert_eq!(first["leases"][0]["expiry"], "expired");
    assert_eq!(first["leases"][1]["expiry"], "invalid");
    assert_eq!(first["leases"][0]["schedulerGeneration"], 7);
    assert_eq!(first["leases"][0]["fenceGeneration"], 2);
    let output = cli(root.path(), &home)
        .args([
            "automations",
            "leases",
            "--json",
            "--automation-id",
            "target",
            "--limit",
            "100",
            "--cursor",
            first["cursor"]["next"].as_str().context("next cursor")?,
        ])
        .output()?;
    assert!(output.status.success(), "CLI lease read failed");
    let second: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(second["leases"].as_array().context("leases")?.len(), 12);
    assert_eq!(second["leases"][0]["occurrenceId"], "lease-100");
    assert_eq!(second["cursor"]["hasMore"], false);
    for value in [&first, &second] {
        let text = value.to_string();
        for private in [
            "private-owner-marker",
            "private-reason-marker",
            "private-invalid-expiry",
        ] {
            assert!(!text.contains(private));
        }
        chrono::DateTime::parse_from_rfc3339(value["observedAt"].as_str().context("observedAt")?)?;
    }
    // Windows currently exposes only its owner pipe and ignores --tcp.
    #[cfg(unix)]
    {
        let body = json!({"action": ACTION, "automationId": "target"}).to_string();
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        write!(stream, "POST /api/v1/actions HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())?;
        let mut response = String::new();
        stream.take(64 * 1024).read_to_string(&mut response)?;
        assert!(response.starts_with("HTTP/1.1 403"));
        assert!(response.contains("AUTHORITY_REQUIRED"));
    }
    assert_eq!(
        snapshot(&conn)?,
        before,
        "lease reads changed durable lifecycle state"
    );
    Ok(())
}
