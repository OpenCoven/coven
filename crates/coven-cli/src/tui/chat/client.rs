//! Daemon-backed chat client for the rich TUI.
//!
//! This module intentionally stays thin: the daemon owns live session launch,
//! cwd validation, input delivery, kill, and structured errors. Local session
//! ritual verbs use the shared store path/timestamp helpers because they are
//! ledger-only mutations.

#[cfg(unix)]
use std::io::Read;
#[cfg(any(unix, windows))]
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(any(unix, windows))]
use anyhow::anyhow;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    api::{EventsResponse, HealthResponse, COVEN_API_NAMED_VERSION},
    current_timestamp, daemon, harness, store, STORE_FILE_NAME,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ChatDaemonStatus {
    Running {
        pid: u32,
    },
    Stale {
        pid: u32,
    },
    #[default]
    Stopped,
    ApiMismatch {
        expected: String,
        actual: String,
    },
    Unavailable {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LaunchRequest {
    pub(crate) id: String,
    pub(crate) project_root: String,
    pub(crate) cwd: String,
    pub(crate) harness: String,
    pub(crate) launch_mode: harness::HarnessLaunchMode,
    pub(crate) prompt: String,
    pub(crate) title: String,
    pub(crate) conversation: Option<harness::ConversationHint>,
    /// Stable per-conversation id used to group multiple chat turns under
    /// one row in `/sessions`. Conceptually distinct from `conversation`
    /// (which drives the harness CLI's own resume args), though in
    /// practice both fields carry the same value for both harnesses:
    /// claude's chat-generated UUID is also the `conversation_id`, and
    /// codex's captured `session id: <uuid>` is reused as the
    /// `conversation_id` once we learn it. See
    /// `docs/chat-persistence.md`.
    pub(crate) conversation_id: Option<String>,
}

impl LaunchRequest {
    pub(crate) fn for_current_dir(harness: &str, prompt: &str) -> Result<Self> {
        let cwd = std::env::current_dir().context("failed to read current directory")?;
        let cwd = cwd.to_string_lossy().into_owned();
        Ok(Self {
            id: Uuid::new_v4().to_string(),
            project_root: cwd.clone(),
            cwd,
            harness: harness.to_string(),
            launch_mode: harness::HarnessLaunchMode::NonInteractive,
            prompt: prompt.to_string(),
            title: session_title(prompt),
            conversation: None,
            conversation_id: None,
        })
    }

    pub(crate) fn with_conversation(mut self, hint: harness::ConversationHint) -> Self {
        self.conversation = Some(hint);
        self
    }

    pub(crate) fn with_conversation_id(mut self, id: String) -> Self {
        self.conversation_id = Some(id);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChatEventQuery<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) after_seq: Option<i64>,
    pub(crate) limit: Option<i64>,
}

pub(crate) trait ChatClient {
    fn daemon_status(&mut self) -> Result<ChatDaemonStatus>;
    fn launch_session(&mut self, request: LaunchRequest) -> Result<store::SessionRecord>;
    fn get_session(&mut self, session_id: &str) -> Result<store::SessionRecord>;
    fn list_sessions(&mut self) -> Result<Vec<store::SessionRecord>>;
    fn list_events(&mut self, query: ChatEventQuery<'_>) -> Result<Vec<store::EventRecord>>;
    fn send_input(&mut self, session_id: &str, data: &str) -> Result<()>;
    fn kill_session(&mut self, session_id: &str) -> Result<()>;
    fn archive_session(&mut self, session_id: &str) -> Result<()>;
    fn summon_session(&mut self, session_id: &str) -> Result<store::SessionRecord>;
    fn sacrifice_session(&mut self, session_id: &str) -> Result<()>;
}

pub(crate) struct DaemonChatClient {
    coven_home: PathBuf,
    api_checked: bool,
}

impl DaemonChatClient {
    /// Resolve a client from the process environment. Fails when no Coven
    /// home can be determined instead of guessing a cwd-relative `.coven`.
    pub(crate) fn detect() -> anyhow::Result<Self> {
        Ok(Self {
            coven_home: crate::paths::coven_home_dir()?,
            api_checked: false,
        })
    }

    /// Construct a client pinned to a specific Coven home directory. Used by
    /// the Cast follower when it needs to spin up a second client on a
    /// background thread without re-detecting `$COVEN_HOME`.
    pub(crate) fn with_coven_home(coven_home: PathBuf) -> Self {
        Self {
            coven_home,
            api_checked: false,
        }
    }
}

impl DaemonChatClient {
    fn store_path(&self) -> PathBuf {
        self.coven_home.join(STORE_FILE_NAME)
    }

    fn open_store(&self) -> Result<rusqlite::Connection> {
        store::open_store(&self.store_path())
    }

    fn request_json<T: for<'de> Deserialize<'de>>(
        &mut self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let response = self.request(method, path, body)?;
        serde_json::from_str(&response.body).with_context(|| {
            format!(
                "failed to parse Coven daemon response for {method} {path}: {}",
                response.body
            )
        })
    }

    fn request_empty(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<()> {
        self.request(method, path, body).map(|_| ())
    }

    fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<HttpResponse> {
        if path != "/api/v1/health" {
            self.ensure_api_contract()?;
        }
        self.raw_request(method, path, body)
    }

    fn raw_request(&self, method: &str, path: &str, body: Option<Value>) -> Result<HttpResponse> {
        request_daemon(&self.coven_home, method, path, body)
    }

    fn ensure_api_contract(&mut self) -> Result<()> {
        if self.api_checked {
            return Ok(());
        }

        let current_exe =
            std::env::current_exe().context("failed to resolve current executable")?;
        daemon::ensure_background_server(&self.coven_home, &current_exe, current_timestamp())
            .context("failed to start Coven daemon")?;
        let response = self.raw_request("GET", "/api/v1/health", None)?;
        let health: HealthResponse = serde_json::from_str(&response.body).with_context(|| {
            format!(
                "failed to parse Coven daemon response for GET /api/v1/health: {}",
                response.body
            )
        })?;
        if health.api_version != COVEN_API_NAMED_VERSION {
            anyhow::bail!(
                "Coven daemon API mismatch: expected {COVEN_API_NAMED_VERSION}, got {}",
                health.api_version
            );
        }
        self.api_checked = true;
        Ok(())
    }
}

impl ChatClient for DaemonChatClient {
    fn daemon_status(&mut self) -> Result<ChatDaemonStatus> {
        match daemon::background_server_status(&self.coven_home)? {
            Some(daemon::DaemonStatusState::Running(status)) => {
                let response = match self.raw_request("GET", "/api/v1/health", None) {
                    Ok(response) => response,
                    Err(error) => {
                        return Ok(ChatDaemonStatus::Unavailable {
                            message: error.to_string(),
                        })
                    }
                };
                let health: HealthResponse =
                    serde_json::from_str(&response.body).with_context(|| {
                        format!(
                            "failed to parse Coven daemon response for GET /api/v1/health: {}",
                            response.body
                        )
                    })?;
                if health.api_version != COVEN_API_NAMED_VERSION {
                    return Ok(ChatDaemonStatus::ApiMismatch {
                        expected: COVEN_API_NAMED_VERSION.to_string(),
                        actual: health.api_version,
                    });
                }
                Ok(ChatDaemonStatus::Running { pid: status.pid })
            }
            Some(daemon::DaemonStatusState::Stale(status)) => {
                Ok(ChatDaemonStatus::Stale { pid: status.pid })
            }
            None => Ok(ChatDaemonStatus::Stopped),
        }
    }

    fn launch_session(&mut self, request: LaunchRequest) -> Result<store::SessionRecord> {
        let mut body = json!({
            "projectRoot": request.project_root,
            "cwd": request.cwd,
            "harness": request.harness,
            "launchMode": match request.launch_mode {
                harness::HarnessLaunchMode::Interactive => "interactive",
                harness::HarnessLaunchMode::NonInteractive => "nonInteractive",
                harness::HarnessLaunchMode::Stream => "stream",
            },
            "prompt": request.prompt,
            "title": request.title,
        });
        if let Some(hint) = request.conversation.as_ref() {
            let (mode, id) = match hint {
                harness::ConversationHint::Init { id } => ("init", id),
                harness::ConversationHint::Resume { id } => ("resume", id),
            };
            body.as_object_mut()
                .expect("json! literal is an object")
                .insert("conversation".to_string(), json!({"mode": mode, "id": id}));
        }
        if let Some(conversation_id) = request.conversation_id.as_ref() {
            body.as_object_mut()
                .expect("json! literal is an object")
                .insert("conversationId".to_string(), json!(conversation_id));
        }
        self.request_json("POST", "/api/v1/sessions", Some(body))
    }

    fn get_session(&mut self, session_id: &str) -> Result<store::SessionRecord> {
        self.request_json("GET", &format!("/api/v1/sessions/{session_id}"), None)
    }

    fn list_sessions(&mut self) -> Result<Vec<store::SessionRecord>> {
        self.request_json("GET", "/api/v1/sessions", None)
    }

    fn list_events(&mut self, query: ChatEventQuery<'_>) -> Result<Vec<store::EventRecord>> {
        let mut path = format!("/api/v1/events?sessionId={}", query.session_id);
        if let Some(after_seq) = query.after_seq {
            path.push_str(&format!("&afterSeq={after_seq}"));
        }
        if let Some(limit) = query.limit {
            path.push_str(&format!("&limit={limit}"));
        }
        let response: EventsResponse = self.request_json("GET", &path, None)?;
        Ok(response.events)
    }

    fn send_input(&mut self, session_id: &str, data: &str) -> Result<()> {
        self.request_empty(
            "POST",
            &format!("/api/v1/sessions/{session_id}/input"),
            Some(json!({ "data": data })),
        )
    }

    fn kill_session(&mut self, session_id: &str) -> Result<()> {
        self.request_empty(
            "POST",
            &format!("/api/v1/sessions/{session_id}/kill"),
            Some(json!({})),
        )
    }

    fn archive_session(&mut self, session_id: &str) -> Result<()> {
        let conn = self.open_store()?;
        let Some(session) = store::get_session(&conn, session_id)? else {
            anyhow::bail!("session `{session_id}` not found");
        };
        if session.status == "running" {
            anyhow::bail!("session `{session_id}` is still running; stop it before archiving");
        }
        store::archive_session(&conn, session_id, &current_timestamp())
    }

    fn summon_session(&mut self, session_id: &str) -> Result<store::SessionRecord> {
        let conn = self.open_store()?;
        let Some(session) = store::get_session(&conn, session_id)? else {
            anyhow::bail!("session `{session_id}` not found");
        };
        if session.archived_at.is_some() {
            store::summon_session(&conn, session_id, &current_timestamp())?;
            let Some(session) = store::get_session(&conn, session_id)? else {
                anyhow::bail!("session `{session_id}` not found");
            };
            return Ok(session);
        }
        Ok(session)
    }

    fn sacrifice_session(&mut self, session_id: &str) -> Result<()> {
        let conn = self.open_store()?;
        let Some(session) = store::get_session(&conn, session_id)? else {
            anyhow::bail!("session `{session_id}` not found");
        };
        if session.status == "running" {
            anyhow::bail!("session `{session_id}` is still running; do not sacrifice live work");
        }
        store::sacrifice_session(&conn, session_id)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct HttpResponse {
    status: u16,
    body: String,
}

#[cfg(unix)]
fn request_daemon(
    coven_home: &Path,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<HttpResponse> {
    use std::os::unix::net::UnixStream;

    let socket = daemon::daemon_socket_path(coven_home);
    let body = body.map(|value| value.to_string()).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let mut stream = UnixStream::connect(&socket).with_context(|| {
        format!(
            "failed to connect to Coven daemon socket {}; run `coven daemon start` and retry",
            socket.display()
        )
    })?;
    stream
        .write_all(request.as_bytes())
        .context("failed to write Coven daemon request")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to finish Coven daemon request")?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .context("failed to read Coven daemon response")?;
    parse_http_response(&response)
}

#[cfg(windows)]
fn request_daemon(
    coven_home: &Path,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<HttpResponse> {
    use interprocess::{
        local_socket::{prelude::*, ConnectOptions, GenericNamespaced},
        ConnectWaitMode,
    };
    use std::time::Duration;

    let body = body.map(|value| value.to_string()).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let pipe_name = daemon::windows_pipe_name(coven_home);
    let name = pipe_name
        .clone()
        .to_ns_name::<GenericNamespaced>()
        .context("failed to parse Coven daemon pipe name")?;
    let mut stream = ConnectOptions::new()
        .name(name)
        .wait_mode(ConnectWaitMode::Timeout(Duration::from_secs(5)))
        .connect_sync()
        .with_context(|| {
            format!(
                "failed to connect to Coven daemon pipe {pipe_name}; run `coven daemon start` and retry"
            )
        })?;
    stream
        .write_all(request.as_bytes())
        .context("failed to write Coven daemon request")?;
    stream
        .flush()
        .context("failed to flush Coven daemon request")?;
    let (status, body) = daemon::read_windows_pipe_http_response(
        stream,
        Duration::from_secs(5),
        daemon::MAX_SOCKET_BODY_BYTES,
    )?;
    let body = String::from_utf8(body).context("Coven daemon response body was not UTF-8")?;
    if !(200..300).contains(&status) {
        return Err(daemon_error(status, &body));
    }
    Ok(HttpResponse { status, body })
}

#[cfg(not(any(unix, windows)))]
fn request_daemon(
    _coven_home: &Path,
    _method: &str,
    _path: &str,
    _body: Option<Value>,
) -> Result<HttpResponse> {
    anyhow::bail!("Coven daemon chat is not implemented on this platform")
}

#[cfg(unix)]
fn parse_http_response(response: &str) -> Result<HttpResponse> {
    let (head, body) = response
        .split_once("\r\n\r\n")
        .or_else(|| response.split_once("\n\n"))
        .context("invalid Coven daemon HTTP response")?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .context("invalid Coven daemon HTTP status")?;
    if !(200..300).contains(&status) {
        return Err(daemon_error(status, body));
    }
    Ok(HttpResponse {
        status,
        body: body.to_string(),
    })
}

#[cfg(any(unix, windows))]
fn daemon_error(status: u16, body: &str) -> anyhow::Error {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if let Some(message) = value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
        {
            return anyhow!("Coven daemon rejected request with HTTP {status}: {message}");
        }
    }
    anyhow!("Coven daemon rejected request with HTTP {status}")
}

fn session_title(prompt: &str) -> String {
    let trimmed = prompt.trim();
    let mut title = String::new();
    for ch in trimmed.chars().take(48) {
        title.push(ch);
    }
    if title.is_empty() {
        "Coven chat".to_string()
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_daemon_client_reads_framed_response_without_waiting_for_pipe_close() -> Result<()> {
        use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions};
        use std::io::{Read, Write};
        use std::thread;
        use std::time::{Duration, Instant};

        let home = tempfile::tempdir()?;
        let name = daemon::windows_pipe_name(home.path())
            .to_ns_name::<GenericNamespaced>()
            .context("pipe name")?;
        let listener = ListenerOptions::new().name(name).create_sync()?;
        let server = thread::spawn(move || -> Result<()> {
            let mut stream = listener.incoming().next().context("accept ended")??;
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte)?;
                request.push(byte[0]);
            }
            stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: keep-alive\r\n\r\n{\"ok\":true}",
            )?;
            stream.flush()?;
            thread::sleep(Duration::from_secs(2));
            Ok(())
        });

        let started = Instant::now();
        let response = request_daemon(home.path(), "GET", "/api/v1/health", None)?;
        assert_eq!(response.status, 200);
        assert_eq!(response.body, r#"{"ok":true}"#);
        assert!(started.elapsed() < Duration::from_millis(1500));
        server.join().expect("server thread")?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn parses_successful_http_response_body() -> Result<()> {
        let response =
            parse_http_response("HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}")?;

        assert_eq!(response.status, 200);
        assert_eq!(response.body, r#"{"ok":true}"#);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn turns_structured_daemon_errors_into_readable_errors() {
        let error = parse_http_response(
            "HTTP/1.1 409 Conflict\r\n\r\n{\"error\":{\"message\":\"Session is not live.\"}}",
        )
        .unwrap_err();

        assert!(error.to_string().contains("Session is not live."));
    }

    #[test]
    fn launch_request_uses_current_dir_and_non_interactive_mode_for_chat() -> Result<()> {
        let request = LaunchRequest::for_current_dir("codex", "summarize")?;

        assert_eq!(request.harness, "codex");
        assert_eq!(request.prompt, "summarize");
        assert_eq!(
            request.launch_mode,
            crate::harness::HarnessLaunchMode::NonInteractive
        );
        assert!(request.conversation.is_none());
        assert!(!request.project_root.is_empty());
        assert_eq!(request.project_root, request.cwd);
        Ok(())
    }

    #[test]
    fn with_conversation_attaches_resume_hint() -> Result<()> {
        let request = LaunchRequest::for_current_dir("claude", "next turn")?.with_conversation(
            crate::harness::ConversationHint::Resume {
                id: "abc-123".to_string(),
            },
        );

        assert_eq!(
            request.conversation,
            Some(crate::harness::ConversationHint::Resume {
                id: "abc-123".to_string()
            })
        );
        Ok(())
    }
}
