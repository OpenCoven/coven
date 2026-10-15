//! Real daemon SSE: cursor replay on owner IPC, and concurrent request admission
//! while all streaming slots are occupied (TCP on Unix, owner pipe on Windows).
#![cfg(any(unix, windows))]
use anyhow::{bail, Context, Result};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

trait Io: Read + Write + Send {}
impl<T: Read + Write + Send> Io for T {}
type Connection = Box<dyn Io>;
struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[cfg(unix)]
fn ipc(home: &Path) -> Result<Connection> {
    let stream = std::os::unix::net::UnixStream::connect(home.join("coven.sock"))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    Ok(Box::new(stream))
}
#[cfg(windows)]
fn ipc(home: &Path) -> Result<Connection> {
    use interprocess::{
        local_socket::{prelude::*, ConnectOptions, GenericNamespaced},
        ConnectWaitMode,
    };
    let status: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join("daemon.json"))?)?;
    let pipe = status["socket"].as_str().context("missing pipe")?;
    Ok(Box::new(
        ConnectOptions::new()
            .name(pipe.to_ns_name::<GenericNamespaced>()?)
            .wait_mode(ConnectWaitMode::Timeout(Duration::from_secs(2)))
            .connect_sync()?,
    ))
}
fn request(
    mut io: Connection,
    route: &str,
    extra: &str,
) -> Result<(String, BufReader<Connection>)> {
    write!(io, "GET {route} HTTP/1.1\r\nHost: localhost\r\n{extra}\r\n")?;
    let mut reader = BufReader::new(io);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            bail!("EOF before headers: {headers}");
        }
        headers.push_str(&line);
        if line == "\r\n" {
            return Ok((headers, reader));
        }
    }
}
fn event(reader: &mut impl BufRead) -> Result<i64> {
    let mut id = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            bail!("EOF before event");
        }
        if let Some(value) = line.strip_prefix("id: ") {
            id = Some(value.trim().parse()?);
        }
        if line == "\n" {
            if let Some(id) = id {
                return Ok(id);
            }
        }
    }
}
fn seed(home: &Path) -> Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open(home.join("coven.sqlite3"))?;
    conn.busy_timeout(Duration::from_secs(30))?;
    conn.execute_batch("INSERT INTO sessions
        (id,project_root,harness,title,status,created_at,updated_at,conversation_id)
        VALUES ('sse-test','/fixture','claude','Home','completed','2026-10-15','2026-10-15','sse-conversation');
        INSERT INTO main_sessions
        (scope_key,harness,conversation_id,current_session_id,created_at,updated_at,project_root)
        VALUES ('instance:main','claude','sse-conversation','sse-test','2026-10-15','2026-10-15','/fixture');")?;
    Ok(conn)
}
fn append(conn: &rusqlite::Connection, id: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO events (id,session_id,kind,payload_json,created_at,redaction_status)
        VALUES (?1,'sse-test','output','{}','2026-10-15','redacted')",
        [id],
    )?;
    Ok(conn.last_insert_rowid())
}
fn run(capacity: bool) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let reservation = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = reservation.local_addr()?;
    drop(reservation);
    let log = temp.path().join("daemon.log");
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_coven"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", temp.path())
            .env("COVEN_HOME", &home)
            .args(["daemon", "serve", "--tcp", &address.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log)?)
            .spawn()?,
    );
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker_home = home.clone();
    let worker = std::thread::spawn(move || {
        let outcome = (|| -> Result<Vec<BufReader<Connection>>> {
            let ready_deadline = Instant::now() + Duration::from_secs(60);
            loop {
                if let Ok((headers, _)) =
                    ipc(&worker_home).and_then(|io| request(io, "/health", ""))
                {
                    if headers.starts_with("HTTP/1.1 200 ") {
                        break;
                    }
                }
                if Instant::now() >= ready_deadline {
                    bail!("daemon did not become ready");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let conn = seed(&worker_home)?;
            let first = append(&conn, "first")?;
            let connect = || -> Result<Connection> {
                #[cfg(unix)]
                if capacity {
                    let stream = std::net::TcpStream::connect(address)?;
                    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
                    return Ok(Box::new(stream));
                }
                ipc(&worker_home)
            };
            if capacity {
                let mut held = Vec::new();
                for _ in 0..16 {
                    let (headers, mut stream) =
                        request(connect()?, "/api/v1/main-session/events", "")?;
                    assert!(headers.starts_with("HTTP/1.1 200 "), "{headers}");
                    assert_eq!(event(&mut stream)?, first);
                    held.push(stream);
                }
                let (headers, _) = request(connect()?, "/api/v1/main-session/events", "")?;
                assert!(headers.starts_with("HTTP/1.1 503 "), "{headers}");
                let (headers, _) = request(connect()?, "/health", "")?;
                assert!(
                    headers.starts_with("HTTP/1.1 200 "),
                    "ordinary requests must remain available: {headers}"
                );
                let (headers, _) = request(ipc(&worker_home)?, "/health", "")?;
                assert!(headers.starts_with("HTTP/1.1 200 "), "{headers}");
                // Stop consuming every stream and publish more than either
                // transport's socket/pipe buffer can absorb. The daemon must
                // release streaming permits instead of retaining slow readers.
                let payload = serde_json::to_string(&"x".repeat(256 * 1024))?;
                conn.execute_batch("BEGIN IMMEDIATE")?;
                for index in 0..64 {
                    conn.execute("INSERT INTO events (id,session_id,kind,payload_json,created_at,redaction_status)
                        VALUES (?1,'sse-test','output',?2,'2026-10-15','redacted')",
                        rusqlite::params![format!("backpressure-{index}"), payload])?;
                }
                conn.execute_batch("COMMIT")?;
                // Hang guard, deliberately far above the five-second frame
                // deadline plus one blocking socket timeout. No timing race is
                // used to sequence the reader: all clients are already paused.
                let deadline = Instant::now() + Duration::from_secs(45);
                loop {
                    let (headers, stream) = request(connect()?, "/api/v1/main-session/events", "")?;
                    if headers.starts_with("HTTP/1.1 200 ") {
                        held.push(stream);
                        break;
                    }
                    assert!(headers.starts_with("HTTP/1.1 503 "), "{headers}");
                    if Instant::now() >= deadline {
                        bail!("slow readers retained all stream permits");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Ok(held)
            } else {
                let second = append(&conn, "second")?;
                let (headers, mut stream) = request(connect()?, "/api/v1/main-session/events", "")?;
                assert!(
                    headers.contains("Content-Type: text/event-stream\r\n"),
                    "{headers}"
                );
                assert!(!headers.contains("Content-Length:"));
                assert_eq!(event(&mut stream)?, first);
                drop(stream);
                let third = append(&conn, "third")?;
                let (_, mut resumed) = request(
                    connect()?,
                    "/api/v1/main-session/events",
                    &format!("Last-Event-ID: {first}\r\n"),
                )?;
                assert_eq!(event(&mut resumed)?, second);
                assert_eq!(event(&mut resumed)?, third);
                let fourth = append(&conn, "fourth")?;
                assert_eq!(event(&mut resumed)?, fourth);
                Ok(vec![resumed])
            }
        })();
        let report = outcome
            .as_ref()
            .map(|_| ())
            .map_err(|error| format!("{error:#}"));
        let _ = done_tx.send(report);
        let _ = release_rx.recv(); // Hold clients open through daemon shutdown.
        drop(outcome);
    });
    // Hang guard for native pipes, whose client reads have no I/O timeout.
    let outcome = done_rx
        .recv_timeout(Duration::from_secs(120))
        .context("SSE conformance worker wedged")?;
    if let Err(error) = outcome {
        let _ = release_tx.send(());
        worker.join().expect("SSE worker panicked");
        bail!("{error}; daemon log: {}", std::fs::read_to_string(&log)?);
    }
    #[cfg(unix)]
    {
        let started = Instant::now();
        // The existing daemon-stop contract is two seconds, including open SSE.
        unsafe {
            libc::kill(daemon.0.id() as i32, libc::SIGTERM);
        }
        let status = loop {
            if let Some(status) = daemon.0.try_wait()? {
                break status;
            }
            if started.elapsed() >= Duration::from_secs(2) {
                bail!(
                    "daemon shutdown with open SSE exceeded two seconds: {:?}",
                    started.elapsed()
                );
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(status.success(), "{status}");
    }
    #[cfg(windows)]
    {
        daemon.0.kill()?;
        daemon.0.wait()?;
    }
    release_tx.send(())?;
    worker.join().expect("SSE worker panicked");
    Ok(())
}
#[test]
fn main_session_events_reconnect_over_owner_ipc() -> Result<()> {
    run(false)
}
#[test]
fn main_session_events_capacity_preserves_requests_and_shutdown() -> Result<()> {
    run(true)
}
