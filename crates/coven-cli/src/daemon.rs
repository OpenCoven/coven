use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

#[cfg(unix)]
use std::io::{BufRead, BufReader, Read};
#[cfg(unix)]
use std::os::unix::net::UnixListener;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    api::{SessionLaunch, SessionRuntime},
    pty_runner,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    pub pid: u32,
    pub started_at: String,
    pub socket: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonSpawnSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub coven_home: PathBuf,
}

pub trait RuntimeKiller: Send {
    fn kill(&mut self) -> Result<()>;
}

#[derive(Default)]
pub struct LiveSessionRuntime {
    coven_home: Option<PathBuf>,
    sessions: Mutex<HashMap<String, LiveSessionHandle>>,
}

struct LiveSessionHandle {
    input: Box<dyn Write + Send>,
    killer: Box<dyn RuntimeKiller>,
}

impl LiveSessionRuntime {
    pub fn with_coven_home(coven_home: PathBuf) -> Self {
        Self {
            coven_home: Some(coven_home),
            sessions: Mutex::default(),
        }
    }

    #[allow(dead_code)]
    pub fn register(
        &self,
        session_id: String,
        input: Box<dyn Write + Send>,
        killer: Box<dyn RuntimeKiller>,
    ) -> Result<()> {
        self.sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("live session registry lock poisoned"))?
            .insert(session_id, LiveSessionHandle { input, killer });
        Ok(())
    }
}

impl SessionRuntime for LiveSessionRuntime {
    fn launch_session(&self, launch: &SessionLaunch) -> Result<()> {
        let command = pty_runner::build_harness_command(
            &launch.harness,
            &launch.prompt,
            Path::new(&launch.cwd),
        )?;
        let observer = self
            .coven_home
            .as_ref()
            .map(|coven_home| output_observer(coven_home.to_path_buf(), launch.id.clone()));
        let detached = pty_runner::spawn_detached_with_observer(&command, observer)?;
        self.register(launch.id.clone(), detached.input, Box::new(detached.killer))
    }

    fn send_input(&self, session_id: &str, payload: &Value) -> Result<()> {
        let data = payload
            .get("data")
            .and_then(Value::as_str)
            .context("input payload requires string field `data`")?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("live session registry lock poisoned"))?;
        let session = sessions
            .get_mut(session_id)
            .with_context(|| format!("session `{session_id}` is not live in this daemon"))?;
        session
            .input
            .write_all(data.as_bytes())
            .context("failed to write input to live session")?;
        session
            .input
            .flush()
            .context("failed to flush live session input")?;
        Ok(())
    }

    fn kill_session(&self, session_id: &str) -> Result<()> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("live session registry lock poisoned"))?;
        let mut session = sessions
            .remove(session_id)
            .with_context(|| format!("session `{session_id}` is not live in this daemon"))?;
        session.killer.kill()
    }
}

impl RuntimeKiller for Box<dyn portable_pty::ChildKiller + Send + Sync> {
    fn kill(&mut self) -> Result<()> {
        self.as_mut().kill().context("failed to kill live session")
    }
}

fn output_observer(coven_home: PathBuf, session_id: String) -> pty_runner::DetachedPtyObserver {
    let output_home = coven_home.clone();
    let output_session_id = session_id.clone();
    let exit_home = coven_home;
    let exit_session_id = session_id;

    pty_runner::DetachedPtyObserver {
        on_output: Box::new(move |chunk| {
            if chunk.is_empty() {
                return;
            }
            let text = String::from_utf8_lossy(&chunk).into_owned();
            let _ = record_session_event(
                &output_home,
                &output_session_id,
                "output",
                json!({ "data": text }),
            );
        }),
        on_exit: Box::new(move |result| {
            let _ = record_session_exit(&exit_home, &exit_session_id, result);
        }),
    }
}

fn record_session_exit(
    coven_home: &Path,
    session_id: &str,
    result: pty_runner::PtyRunResult,
) -> Result<()> {
    let conn = crate::store::open_store(&coven_home.join("coven.sqlite3"))?;
    if crate::store::get_session(&conn, session_id)?
        .map(|session| session.status == "running")
        .unwrap_or(false)
    {
        crate::store::update_session_status(
            &conn,
            session_id,
            result.status,
            result.exit_code,
            &crate::api::current_timestamp(),
        )?;
    }
    crate::store::insert_event(
        &conn,
        &crate::store::EventRecord {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            kind: "exit".to_string(),
            payload_json: serde_json::to_string(&json!({
                "status": result.status,
                "exitCode": result.exit_code,
            }))
            .context("failed to serialize exit event payload")?,
            created_at: crate::api::current_timestamp(),
        },
    )
}

fn record_session_event(
    coven_home: &Path,
    session_id: &str,
    kind: &str,
    payload: Value,
) -> Result<()> {
    let conn = crate::store::open_store(&coven_home.join("coven.sqlite3"))?;
    crate::store::insert_event(
        &conn,
        &crate::store::EventRecord {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            payload_json: serde_json::to_string(&payload)
                .context("failed to serialize session event payload")?,
            created_at: crate::api::current_timestamp(),
        },
    )
}

pub fn daemon_status_path(coven_home: &Path) -> PathBuf {
    coven_home.join("daemon.json")
}

pub fn daemon_socket_path(coven_home: &Path) -> PathBuf {
    coven_home.join("coven.sock")
}

pub fn background_server_spec(current_exe: &Path, coven_home: &Path) -> DaemonSpawnSpec {
    DaemonSpawnSpec {
        program: current_exe.to_path_buf(),
        args: vec!["daemon".to_string(), "serve".to_string()],
        coven_home: coven_home.to_path_buf(),
    }
}

pub fn start_background_server(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
) -> Result<DaemonStatus> {
    let spec = background_server_spec(current_exe, coven_home);
    std::fs::create_dir_all(coven_home)
        .with_context(|| format!("failed to create Coven home {}", coven_home.display()))?;
    let child = Command::new(&spec.program)
        .args(&spec.args)
        .env("COVEN_HOME", &spec.coven_home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start Coven daemon {}", spec.program.display()))?;
    let status = DaemonStatus {
        pid: child.id(),
        started_at,
        socket: daemon_socket_path(coven_home)
            .to_string_lossy()
            .into_owned(),
    };
    write_status(coven_home, &status)?;
    Ok(status)
}

pub fn recover_orphaned_sessions(coven_home: &Path, updated_at: &str) -> Result<usize> {
    let conn = crate::store::open_store(&coven_home.join("coven.sqlite3"))?;
    crate::store::mark_running_sessions_orphaned(&conn, updated_at)
}

pub fn write_status(coven_home: &Path, status: &DaemonStatus) -> Result<()> {
    std::fs::create_dir_all(coven_home)
        .with_context(|| format!("failed to create Coven home {}", coven_home.display()))?;
    let json = serde_json::to_string_pretty(status).context("failed to serialize daemon status")?;
    std::fs::write(daemon_status_path(coven_home), format!("{json}\n"))
        .context("failed to write daemon status")?;
    Ok(())
}

pub fn read_status(coven_home: &Path) -> Result<Option<DaemonStatus>> {
    let path = daemon_status_path(coven_home);
    if !path.exists() {
        return Ok(None);
    }

    let json = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read daemon status {}", path.display()))?;
    let status = serde_json::from_str(&json).context("failed to parse daemon status")?;
    Ok(Some(status))
}

pub fn clear_status(coven_home: &Path) -> Result<bool> {
    let path = daemon_status_path(coven_home);
    if !path.exists() {
        return Ok(false);
    }

    std::fs::remove_file(&path)
        .with_context(|| format!("failed to remove daemon status {}", path.display()))?;
    Ok(true)
}

pub fn stop_background_server(coven_home: &Path) -> Result<bool> {
    let status = read_status(coven_home)?;
    let Some(status) = status else {
        return Ok(false);
    };

    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(status.pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    clear_status(coven_home)?;
    let socket = daemon_socket_path(coven_home);
    if socket.exists() {
        std::fs::remove_file(&socket)
            .with_context(|| format!("failed to remove daemon socket {}", socket.display()))?;
    }
    Ok(true)
}

#[cfg(unix)]
pub fn bind_api_socket(coven_home: &Path) -> Result<UnixListener> {
    std::fs::create_dir_all(coven_home)
        .with_context(|| format!("failed to create Coven home {}", coven_home.display()))?;
    let socket_path = daemon_socket_path(coven_home);
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)
            .with_context(|| format!("failed to remove stale socket {}", socket_path.display()))?;
    }
    UnixListener::bind(&socket_path)
        .with_context(|| format!("failed to bind Coven API socket {}", socket_path.display()))
}

#[cfg(unix)]
pub fn serve_forever(coven_home: &Path, started_at: String) -> Result<()> {
    let status = DaemonStatus {
        pid: std::process::id(),
        started_at: started_at.clone(),
        socket: daemon_socket_path(coven_home)
            .to_string_lossy()
            .into_owned(),
    };
    write_status(coven_home, &status)?;
    recover_orphaned_sessions(coven_home, &started_at)?;
    let listener = bind_api_socket(coven_home)?;
    let runtime = LiveSessionRuntime::with_coven_home(coven_home.to_path_buf());
    loop {
        serve_next_connection(&listener, coven_home, Some(status.clone()), &runtime)?;
    }
}

#[cfg(unix)]
pub fn serve_next_connection(
    listener: &UnixListener,
    coven_home: &Path,
    status: Option<DaemonStatus>,
    runtime: &dyn SessionRuntime,
) -> Result<()> {
    let (stream, _) = listener
        .accept()
        .context("failed to accept API connection")?;
    let mut reader = BufReader::new(stream);
    let request_line = read_http_request_line(&mut reader)?;
    let content_length = read_http_headers(&mut reader)?;
    let body = read_http_body(&mut reader, content_length)?;
    let mut stream = reader.into_inner();
    let (method, path) = parse_request_line(&request_line)?;
    let response = crate::api::handle_request_with_runtime(
        method,
        path,
        coven_home,
        status,
        body.as_deref(),
        runtime,
    )?;
    let reason = match response.status {
        200 => "OK",
        202 => "Accepted",
        409 => "Conflict",
        404 => "Not Found",
        _ => "OK",
    };
    let http = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status,
        reason,
        response.content_type,
        response.body.len(),
        response.body
    );
    stream
        .write_all(http.as_bytes())
        .context("failed to write API response")?;
    Ok(())
}

#[cfg(unix)]
fn read_http_request_line<R: BufRead>(reader: &mut R) -> Result<String> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("failed to read API request line")?;
    if line.is_empty() {
        anyhow::bail!("empty API request");
    }
    Ok(line)
}

#[cfg(unix)]
fn read_http_headers<R: BufRead>(reader: &mut R) -> Result<usize> {
    let mut content_length = 0;
    let mut header = String::new();
    loop {
        header.clear();
        let bytes = reader
            .read_line(&mut header)
            .context("failed to read API request header")?;
        if bytes == 0 || header == "\r\n" || header == "\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value
                    .trim()
                    .parse()
                    .context("invalid Content-Length header")?;
            }
        }
    }
    Ok(content_length)
}

#[cfg(unix)]
fn read_http_body<R: Read>(reader: &mut R, content_length: usize) -> Result<Option<String>> {
    if content_length == 0 {
        return Ok(None);
    }
    let mut bytes = vec![0; content_length];
    reader
        .read_exact(&mut bytes)
        .context("failed to read API request body")?;
    String::from_utf8(bytes)
        .map(Some)
        .context("API request body was not valid UTF-8")
}

#[cfg(unix)]
fn parse_request_line(line: &str) -> Result<(&str, &str)> {
    let mut parts = line.split_whitespace();
    let method = parts.next().context("missing HTTP method")?;
    let path = parts.next().context("missing HTTP path")?;
    Ok((method, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_runtime_writes_input_to_registered_session() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        let output = SharedBuffer::default();
        runtime.register(
            "session-1".to_string(),
            Box::new(output.clone()),
            Box::new(RecordingKiller::default()),
        )?;

        SessionRuntime::send_input(
            &runtime,
            "session-1",
            &serde_json::json!({ "data": "hello live pty" }),
        )?;

        assert_eq!(output.text(), "hello live pty");
        Ok(())
    }

    #[test]
    fn live_runtime_kills_and_removes_registered_session() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        let killed = std::sync::Arc::new(std::sync::Mutex::new(false));
        runtime.register(
            "session-1".to_string(),
            Box::new(SharedBuffer::default()),
            Box::new(RecordingKiller {
                killed: killed.clone(),
            }),
        )?;

        SessionRuntime::kill_session(&runtime, "session-1")?;

        assert!(*killed.lock().unwrap());
        assert!(SessionRuntime::kill_session(&runtime, "session-1")
            .unwrap_err()
            .to_string()
            .contains("not live"));
        Ok(())
    }

    #[derive(Clone, Default)]
    struct SharedBuffer {
        data: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl SharedBuffer {
        fn text(&self) -> String {
            String::from_utf8(self.data.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.data.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct RecordingKiller {
        killed: std::sync::Arc<std::sync::Mutex<bool>>,
    }

    impl RuntimeKiller for RecordingKiller {
        fn kill(&mut self) -> Result<()> {
            *self.killed.lock().unwrap() = true;
            Ok(())
        }
    }

    #[test]
    fn recovers_persisted_running_sessions_as_orphaned() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let mut running = session_record("running");
        running.status = "running".to_string();
        let mut killed = session_record("killed");
        killed.status = "killed".to_string();
        crate::store::insert_session(&conn, &running)?;
        crate::store::insert_session(&conn, &killed)?;
        drop(conn);

        let updated = recover_orphaned_sessions(temp_dir.path(), "2026-04-27T08:00:00Z")?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let sessions = crate::store::list_sessions(&conn)?;

        assert_eq!(updated, 1);
        assert_eq!(session_status(&sessions, "running"), "orphaned");
        assert_eq!(session_status(&sessions, "killed"), "killed");
        Ok(())
    }

    #[test]
    fn writes_reads_and_clears_daemon_status() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: temp_dir
                .path()
                .join("coven.sock")
                .to_string_lossy()
                .into_owned(),
        };

        write_status(temp_dir.path(), &status)?;

        assert_eq!(read_status(temp_dir.path())?, Some(status));
        assert!(clear_status(temp_dir.path())?);
        assert_eq!(read_status(temp_dir.path())?, None);
        assert!(!clear_status(temp_dir.path())?);
        Ok(())
    }

    #[test]
    fn builds_background_server_spawn_spec() {
        let spec = background_server_spec(
            Path::new("/usr/local/bin/coven"),
            Path::new("/tmp/coven-home"),
        );

        assert_eq!(spec.program, PathBuf::from("/usr/local/bin/coven"));
        assert_eq!(spec.args, vec!["daemon".to_string(), "serve".to_string()]);
        assert_eq!(spec.coven_home, PathBuf::from("/tmp/coven-home"));
    }

    #[cfg(unix)]
    #[test]
    fn serves_health_over_unix_socket() -> Result<()> {
        use std::io::{Read, Write};
        use std::net::Shutdown;
        use std::os::unix::net::UnixStream;
        use std::thread;

        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };
        let listener = bind_api_socket(temp_dir.path())?;
        let home = temp_dir.path().to_path_buf();
        let runtime = LiveSessionRuntime::default();
        let server =
            thread::spawn(move || serve_next_connection(&listener, &home, Some(status), &runtime));

        let mut stream = UnixStream::connect(daemon_socket_path(temp_dir.path()))?;
        stream.write_all(b"GET /health HTTP/1.1\r\nHost: coven\r\n\r\n")?;
        stream.shutdown(Shutdown::Write)?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;

        server.join().expect("server thread panicked")?;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains(r#""ok":true"#));
        assert!(response.contains(r#""pid":12345"#));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn forwards_http_request_body_to_api() -> Result<()> {
        use std::io::{Read, Write};
        use std::net::Shutdown;
        use std::os::unix::net::UnixStream;
        use std::thread;

        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        crate::store::insert_session(
            &conn,
            &crate::store::SessionRecord {
                id: "session-1".to_string(),
                project_root: "/repo".to_string(),
                harness: "codex".to_string(),
                title: "hello from coven".to_string(),
                status: "running".to_string(),
                exit_code: None,
                created_at: "2026-04-27T10:00:00Z".to_string(),
                updated_at: "2026-04-27T10:00:00Z".to_string(),
            },
        )?;
        let listener = bind_api_socket(temp_dir.path())?;
        let home = temp_dir.path().to_path_buf();
        let runtime = LiveSessionRuntime::default();
        runtime.register(
            "session-1".to_string(),
            Box::new(SharedBuffer::default()),
            Box::new(RecordingKiller::default()),
        )?;
        let server = thread::spawn(move || serve_next_connection(&listener, &home, None, &runtime));

        let body = r#"{"data":"hello over socket"}"#;
        let request = format!(
            "POST /sessions/session-1/input HTTP/1.1\r\nHost: coven\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let mut stream = UnixStream::connect(daemon_socket_path(temp_dir.path()))?;
        stream.write_all(request.as_bytes())?;
        stream.shutdown(Shutdown::Write)?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;

        server.join().expect("server thread panicked")?;
        let events = crate::store::list_events(&conn, "session-1")?;
        assert!(response.starts_with("HTTP/1.1 202 Accepted"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "input");
        assert!(events[0].payload_json.contains("hello over socket"));
        Ok(())
    }

    #[test]
    fn records_output_and_exit_events_for_live_session() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let mut session = session_record("session-1");
        session.status = "running".to_string();
        crate::store::insert_session(&conn, &session)?;
        drop(conn);

        record_session_event(
            temp_dir.path(),
            "session-1",
            "output",
            json!({ "data": "hello from pty" }),
        )?;
        record_session_exit(
            temp_dir.path(),
            "session-1",
            pty_runner::PtyRunResult {
                status: "completed",
                exit_code: Some(0),
            },
        )?;

        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let sessions = crate::store::list_sessions(&conn)?;
        let events = crate::store::list_events(&conn, "session-1")?;
        assert_eq!(session_status(&sessions, "session-1"), "completed");
        assert_eq!(events.len(), 2);
        let output = events.iter().find(|event| event.kind == "output").unwrap();
        let exit = events.iter().find(|event| event.kind == "exit").unwrap();
        assert!(output.payload_json.contains("hello from pty"));
        assert!(exit.payload_json.contains("completed"));
        Ok(())
    }

    #[test]
    fn exit_event_does_not_overwrite_killed_session_status() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let mut session = session_record("session-1");
        session.status = "killed".to_string();
        crate::store::insert_session(&conn, &session)?;
        drop(conn);

        record_session_exit(
            temp_dir.path(),
            "session-1",
            pty_runner::PtyRunResult {
                status: "failed",
                exit_code: Some(1),
            },
        )?;

        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let sessions = crate::store::list_sessions(&conn)?;
        assert_eq!(session_status(&sessions, "session-1"), "killed");
        Ok(())
    }

    fn session_record(id: &str) -> crate::store::SessionRecord {
        crate::store::SessionRecord {
            id: id.to_string(),
            project_root: "/repo".to_string(),
            harness: "codex".to_string(),
            title: format!("Session {id}"),
            status: "running".to_string(),
            exit_code: None,
            created_at: "2026-04-27T07:00:00Z".to_string(),
            updated_at: "2026-04-27T07:00:00Z".to_string(),
        }
    }

    fn session_status(sessions: &[crate::store::SessionRecord], id: &str) -> String {
        sessions
            .iter()
            .find(|session| session.id == id)
            .map(|session| session.status.clone())
            .unwrap_or_default()
    }
}
