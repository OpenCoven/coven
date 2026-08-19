#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use coven_client::{
    probe_unix_daemon_health, shutdown_unix_daemon, ClientError, DaemonClient, DaemonEndpoint,
    LifecycleDaemonStatus, ReadEndpoint, UnixDaemonShutdown, WriteEndpoint, PROTOCOL_VERSION,
};

const HEALTH: &str = include_str!("../fixtures/health.json");
const ERROR: &str = include_str!("../fixtures/error.json");

struct TestHome {
    path: PathBuf,
}

impl TestHome {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workspace root");
        let path = workspace.join("c").join(format!(
            "{:x}{:x}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create test Coven home");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .expect("make test Coven home private");
        }
        Self { path }
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
        let _ = fs::remove_dir(self.path.parent().expect("test root"));
    }
}

#[cfg(unix)]
fn serve_once(home: &Path, status: u16, body: String) -> std::thread::JoinHandle<()> {
    serve_responses(home, vec![("/api/v1/health".to_owned(), status, body)])
}

#[cfg(unix)]
fn serve_responses(
    home: &Path,
    responses: Vec<(String, u16, String)>,
) -> std::thread::JoinHandle<()> {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
    };

    let socket = home.join("coven.sock");
    let listener = UnixListener::bind(&socket).expect("bind test daemon socket");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make test daemon socket owner-only");
    std::thread::spawn(move || {
        for (expected_path, status, body) in responses {
            let (mut stream, _) = listener.accept().expect("accept client request");
            let mut request = String::new();
            stream
                .read_to_string(&mut request)
                .expect("read client request");
            if expected_path == "<peer-check>" {
                assert!(
                    request.is_empty(),
                    "capability peer check must not send request bytes"
                );
                continue;
            }
            assert!(
                request.starts_with(&format!("GET {expected_path} HTTP/1.1\r\n"))
                    || request.starts_with(&format!("POST {expected_path} HTTP/1.1\r\n"))
            );
            let response = format!(
                "HTTP/1.1 {status} test\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write daemon response");
        }
    })
}

#[cfg(unix)]
fn serve_raw_response_then_hold(
    home: &Path,
    response: String,
    hold_open: std::time::Duration,
) -> std::thread::JoinHandle<()> {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
    };

    let socket = home.join("coven.sock");
    let listener = UnixListener::bind(&socket).expect("bind test daemon socket");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make test daemon socket owner-only");
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept client request");
        let mut request = String::new();
        stream
            .read_to_string(&mut request)
            .expect("read client request");
        stream
            .write_all(response.as_bytes())
            .expect("write daemon response");
        stream.flush().expect("flush daemon response");
        std::thread::sleep(hold_open);
    })
}

fn health_with_capabilities(capabilities: serde_json::Value) -> String {
    let mut health: serde_json::Value = serde_json::from_str(HEALTH).expect("parse health fixture");
    health["capabilities"] = capabilities;
    health.to_string()
}

#[test]
fn lifecycle_client_rejects_non_utf8_home_before_creating_daemon_artifacts() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, time::Duration};

    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root");
    let mut home_name = format!("client-{}-", std::process::id()).into_bytes();
    home_name.push(0xff);
    let home = workspace.join("c").join(OsString::from_vec(home_name));

    let probe_error = probe_unix_daemon_health(&home, Duration::from_millis(10))
        .expect_err("lifecycle discovery must reject a non-UTF-8 profile");
    assert!(
        probe_error.to_string().contains("valid UTF-8"),
        "probe error must explain the JSON path requirement: {probe_error}"
    );

    let expected = LifecycleDaemonStatus {
        pid: std::process::id(),
        started_at: "2026-08-16T18:29:29Z".to_owned(),
        socket: "unpublished".to_owned(),
    };
    let shutdown_error = shutdown_unix_daemon(&home, &expected, Duration::from_millis(10))
        .expect_err("shutdown discovery must reject a non-UTF-8 profile");
    assert!(
        shutdown_error.to_string().contains("valid UTF-8"),
        "shutdown error must explain the JSON path requirement: {shutdown_error}"
    );
    assert!(!home.join("coven.sock").exists());
    assert!(!home.join("daemon.json").exists());
}

#[cfg(unix)]
fn serve_health_then_stall_write(home: &Path) -> std::thread::JoinHandle<()> {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
    };

    let socket = home.join("coven.sock");
    let listener = UnixListener::bind(&socket).expect("bind test daemon socket");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make test daemon socket owner-only");
    std::thread::spawn(move || {
        // First connection: negotiate health normally, over its own socket
        // connection (the client never reuses connections).
        let (mut stream, _) = listener.accept().expect("accept health request");
        let mut request = String::new();
        stream
            .read_to_string(&mut request)
            .expect("read health request");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            HEALTH.len(),
            HEALTH
        );
        stream
            .write_all(response.as_bytes())
            .expect("write health response");
        drop(stream);

        // Second connection: accept but deliberately never read from it, so
        // a large dependent request's write fills the kernel socket buffer
        // (8KB by default on macOS/Linux) and cannot complete without the
        // client-side write deadline.
        let (stream, _) = listener.accept().expect("accept stalled request");
        std::thread::sleep(std::time::Duration::from_secs(10));
        drop(stream);
    })
}

#[cfg(unix)]
#[test]
fn oversized_request_bodies_are_rejected_before_any_connection_attempt() {
    let home = TestHome::new();
    let server = serve_once(&home.path, 200, HEALTH.to_string());
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    client.health().expect("negotiate health");

    let oversized_body = serde_json::json!({ "padding": "a".repeat(5 * 1024 * 1024) });
    let error = client
        .post_empty(
            WriteEndpoint::SessionInput {
                session_id: "session-1".to_owned(),
            },
            &oversized_body,
        )
        .expect_err(
            "an oversized body must be rejected before any I/O, since the daemon would \
             otherwise reset the connection instead of returning a structured 413",
        );

    assert!(matches!(
        error,
        ClientError::RequestTooLarge { max_bytes, .. } if max_bytes == 4 * 1024 * 1024
    ));

    // Only the health request above should have reached the daemon: the
    // oversized POST must never open a second connection, so the server
    // thread (which only expects a single accept()) joins cleanly instead
    // of hanging on a second `accept()` that never arrives.
    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn a_stalled_daemon_connection_bounds_the_write_phase_of_a_dependent_request() {
    let home = TestHome::new();
    let server = serve_health_then_stall_write(&home.path);
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    client
        .health()
        .expect("negotiate health over the first connection");

    // Larger than the platform's default AF_UNIX socket buffer (8KB on
    // macOS/Linux, and comfortably under the 4MiB request-body cap), so the
    // write cannot complete without the peer ever reading it.
    let body = serde_json::json!({ "padding": "a".repeat(256 * 1024) });
    let started = std::time::Instant::now();

    let error = client
        .post_empty(
            WriteEndpoint::SessionInput {
                session_id: "session-1".to_owned(),
            },
            &body,
        )
        .expect_err(
            "a daemon that accepts a connection but never reads from it must not block \
             the caller's write forever",
        );

    assert!(matches!(error, ClientError::Io { .. }));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "the write phase must be bounded by the request deadline, not the server's 10s stall"
    );

    // The server thread's second `accept()` only unblocks once the client
    // above drops its connection (after timing out) or the process exits;
    // deliberately do not join it here to avoid re-waiting out its sleep.
    drop(server);
}

#[cfg(unix)]
#[test]
fn negotiates_the_v1_health_contract_and_structured_errors() {
    let home = TestHome::new();
    let server = serve_once(&home.path, 200, HEALTH.to_string());
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    let health = client.health().expect("negotiate health");

    assert_eq!(health.api_version, PROTOCOL_VERSION);
    assert!(health.capabilities.structured_errors);
    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn missing_sessions_capability_blocks_sessions_but_not_events() {
    let home = TestHome::new();
    let health = health_with_capabilities(serde_json::json!({
        "events": true,
        "eventCursor": "sequence",
        "structuredErrors": true
    }));
    let server = serve_responses(
        &home.path,
        vec![
            ("/api/v1/health".to_owned(), 200, health),
            ("<peer-check>".to_owned(), 0, String::new()),
            (
                "/api/v1/events?sessionId=session-1".to_owned(),
                200,
                r#"{"events":[]}"#.to_owned(),
            ),
        ],
    );
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    client.health().expect("health remains a valid v1 response");
    let error = client
        .get_json::<serde_json::Value>(coven_client::ReadEndpoint::Sessions { limit: None })
        .expect_err("sessions must be blocked before request transmission");
    assert!(
        error.to_string().contains("capabilities.sessions"),
        "unexpected error: {error}"
    );

    let events: serde_json::Value = client
        .get_json(coven_client::ReadEndpoint::Events {
            session_id: "session-1".to_owned(),
            after_seq: None,
            limit: None,
        })
        .expect("unrelated supported events remain usable");
    assert_eq!(events["events"], serde_json::json!([]));
    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn missing_events_capability_blocks_events_but_not_sessions() {
    let home = TestHome::new();
    let health = health_with_capabilities(serde_json::json!({
        "sessions": true,
        "eventCursor": "sequence",
        "structuredErrors": true
    }));
    let server = serve_responses(
        &home.path,
        vec![
            ("/api/v1/health".to_owned(), 200, health),
            ("<peer-check>".to_owned(), 0, String::new()),
            (
                "/api/v1/sessions".to_owned(),
                200,
                r#"{"sessions":[]}"#.to_owned(),
            ),
        ],
    );
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    client.health().expect("health remains a valid v1 response");
    let error = client
        .get_json::<serde_json::Value>(coven_client::ReadEndpoint::Events {
            session_id: "session-1".to_owned(),
            after_seq: None,
            limit: None,
        })
        .expect_err("events must be blocked before request transmission");
    assert!(
        error.to_string().contains("capabilities.events"),
        "unexpected error: {error}"
    );

    let sessions: serde_json::Value = client
        .get_json(coven_client::ReadEndpoint::Sessions { limit: None })
        .expect("unrelated supported sessions remain usable");
    assert_eq!(sessions["sessions"], serde_json::json!([]));
    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn missing_event_cursor_blocks_cursor_reads_but_not_plain_event_reads() {
    let home = TestHome::new();
    let health = health_with_capabilities(serde_json::json!({
        "sessions": true,
        "events": true,
        "structuredErrors": true
    }));
    let server = serve_responses(
        &home.path,
        vec![
            ("/api/v1/health".to_owned(), 200, health),
            ("<peer-check>".to_owned(), 0, String::new()),
            (
                "/api/v1/events?sessionId=session-1".to_owned(),
                200,
                r#"{"events":[]}"#.to_owned(),
            ),
        ],
    );
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    client.health().expect("health remains a valid v1 response");
    let error = client
        .get_json::<serde_json::Value>(coven_client::ReadEndpoint::Events {
            session_id: "session-1".to_owned(),
            after_seq: Some(7),
            limit: None,
        })
        .expect_err("cursor reads must be blocked before request transmission");
    assert!(
        error.to_string().contains("capabilities.eventCursor"),
        "unexpected error: {error}"
    );

    let events: serde_json::Value = client
        .get_json(coven_client::ReadEndpoint::Events {
            session_id: "session-1".to_owned(),
            after_seq: None,
            limit: None,
        })
        .expect("plain reads do not require the cursor capability");
    assert_eq!(events["events"], serde_json::json!([]));
    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn rejects_an_incompatible_health_version() {
    let home = TestHome::new();
    let server = serve_once(
        &home.path,
        200,
        HEALTH.replace(PROTOCOL_VERSION, "coven.daemon.v2"),
    );
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    let error = client.health().expect_err("reject incompatible protocol");

    assert!(matches!(
        error,
        ClientError::ProtocolVersion {
            expected,
            actual
        } if expected == PROTOCOL_VERSION && actual == "coven.daemon.v2"
    ));
    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn preserves_structured_daemon_error_fields() {
    let home = TestHome::new();
    let server = serve_once(&home.path, 409, ERROR.to_string());
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    let error = client.health().expect_err("preserve daemon error");

    assert!(matches!(
        error,
        ClientError::Daemon {
            status: 409,
            error
        } if error.code == "session_not_live"
            && error.message == "Session is not live."
            && error.details == serde_json::json!({ "sessionId": "session-1" })
    ));
    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn preserves_http_status_for_an_unstructured_daemon_error() {
    let home = TestHome::new();
    let server = serve_once(&home.path, 502, "upstream unavailable".to_owned());
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    let error = client
        .health()
        .expect_err("preserve unstructured daemon status");

    assert!(matches!(error, ClientError::HttpStatus(502)));
    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn discovers_only_an_owner_local_unix_socket() {
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};

    let home = TestHome::new();
    let socket = home.path.join("coven.sock");
    let _listener = UnixListener::bind(&socket).expect("bind test daemon socket");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make test daemon socket owner-only");

    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");

    assert!(endpoint.is_owner_local());
}

#[cfg(unix)]
#[test]
fn reads_a_framed_health_fixture_without_waiting_for_socket_close() {
    let home = TestHome::new();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        HEALTH.len(),
        HEALTH
    );
    let server =
        serve_raw_response_then_hold(&home.path, response, std::time::Duration::from_secs(1));
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);
    let started = std::time::Instant::now();

    let health = client.health().expect("read complete framed response");

    assert_eq!(health.api_version, PROTOCOL_VERSION);
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn rejects_bytes_buffered_after_an_ordinary_framed_response() {
    let home = TestHome::new();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}X",
        HEALTH.len(),
        HEALTH
    );
    let server =
        serve_raw_response_then_hold(&home.path, response, std::time::Duration::from_millis(0));
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    let error = client
        .health()
        .expect_err("reject a byte after the complete response frame");

    assert!(matches!(
        error,
        ClientError::InvalidHttpResponse(message)
            if message.contains("bytes after") && message.contains("response completed")
    ));
    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn rejects_a_trailing_byte_queued_with_the_body_after_a_header_only_read() {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        time::Duration,
    };

    let home = TestHome::new();
    let socket = home.path.join("coven.sock");
    let listener = UnixListener::bind(&socket).expect("bind test daemon socket");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make test daemon socket owner-only");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept client request");
        let mut request = String::new();
        stream
            .read_to_string(&mut request)
            .expect("read client request");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            HEALTH.len()
        )
        .expect("write response headers");
        stream.flush().expect("flush response headers");
        std::thread::sleep(Duration::from_millis(20));
        write!(stream, "{HEALTH}X").expect("write body and trailing byte");
    });
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    let error = client
        .health()
        .expect_err("reject a trailing byte queued with a later body read");

    assert!(matches!(
        error,
        ClientError::InvalidHttpResponse(message)
            if message.contains("bytes after") && message.contains("response completed")
    ));
    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn rejects_chunked_health_responses_when_chunked_framing_is_unsupported() {
    let home = TestHome::new();
    let response =
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n{\r\n0\r\n\r\n".to_owned();
    let server =
        serve_raw_response_then_hold(&home.path, response, std::time::Duration::from_millis(0));
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    let error = client.health().expect_err("reject chunked framing");

    assert!(matches!(
        error,
        ClientError::InvalidHttpResponse(message) if message.contains("Transfer-Encoding")
    ));
    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn health_framing_stays_stable_under_parallel_requests() {
    let clients = (0..16)
        .map(|_| {
            std::thread::spawn(|| {
                let home = TestHome::new();
                let server = serve_once(&home.path, 200, HEALTH.to_owned());
                let endpoint =
                    DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
                let mut client = DaemonClient::new(endpoint);

                assert_eq!(
                    client.health().expect("negotiate health").api_version,
                    PROTOCOL_VERSION
                );
                server.join().expect("server thread");
            })
        })
        .collect::<Vec<_>>();

    for client in clients {
        client.join().expect("parallel client");
    }
}

#[cfg(unix)]
#[test]
fn a_failed_renegotiation_blocks_dependent_requests_until_health_succeeds_again() {
    let health_ok = HEALTH.to_owned();
    let mut not_ready: serde_json::Value = serde_json::from_str(HEALTH).expect("parse fixture");
    not_ready["ok"] = serde_json::json!(false);
    let health_not_ready = not_ready.to_string();

    let home = TestHome::new();
    let server = serve_responses(
        &home.path,
        vec![
            ("/api/v1/health".to_owned(), 200, health_ok),
            ("/api/v1/health".to_owned(), 200, health_not_ready.clone()),
            // A dependent request must trigger its own revalidation instead
            // of trusting the stale success from the first call; if it does,
            // this third accept() observes another /api/v1/health hit (which
            // also reports not-ready) rather than the kill request below.
            ("/api/v1/health".to_owned(), 200, health_not_ready),
        ],
    );
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    client
        .health()
        .expect("first health negotiates successfully");
    let renegotiation_error = client
        .health()
        .expect_err("second health call reports the daemon is not ready");
    assert!(matches!(renegotiation_error, ClientError::HealthNotReady));

    let dependent_error = client
        .post_empty(
            WriteEndpoint::SessionKill {
                session_id: "session-1".to_owned(),
            },
            &serde_json::json!({}),
        )
        .expect_err("a dependent request must not proceed on stale negotiated state");
    assert!(matches!(dependent_error, ClientError::HealthNotReady));

    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn a_disappeared_cached_socket_preserves_the_connect_error_contract() {
    let home = TestHome::new();
    let original = serve_once(&home.path, 200, HEALTH.to_owned());
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover original daemon");
    let mut client = DaemonClient::new(endpoint);
    client.health().expect("negotiate original daemon");
    original.join().expect("original daemon thread");
    fs::remove_file(home.path.join("coven.sock")).expect("remove original daemon endpoint");

    let error = client
        .get_json::<serde_json::Value>(ReadEndpoint::Sessions { limit: None })
        .expect_err("a disappeared endpoint must fail before request bytes");

    assert!(matches!(
        error,
        ClientError::Io {
            operation: "failed to connect to Coven daemon socket",
            ..
        }
    ));
}

#[cfg(unix)]
#[test]
fn a_mutation_is_not_sent_to_a_replacement_before_that_peer_is_negotiated() {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
    };

    let home = TestHome::new();
    let original = serve_once(&home.path, 200, HEALTH.to_owned());
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover original daemon");
    let mut client = DaemonClient::new(endpoint);
    client.health().expect("negotiate original daemon");
    original.join().expect("original daemon thread");

    let socket = home.path.join("coven.sock");
    fs::remove_file(&socket).expect("remove original daemon endpoint");
    let replacement = UnixListener::bind(&socket).expect("bind replacement daemon");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make replacement endpoint owner-only");
    let server = std::thread::spawn(move || {
        let (mut first, _) = replacement.accept().expect("accept peer check");
        let mut first_request = String::new();
        first
            .read_to_string(&mut first_request)
            .expect("read peer-check connection");
        if !first_request.is_empty() {
            first
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .expect("finish incorrectly transmitted mutation");
            return vec![first_request];
        }

        let (mut health, _) = replacement.accept().expect("accept replacement health");
        let mut health_request = String::new();
        health
            .read_to_string(&mut health_request)
            .expect("read replacement health");
        let health_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            HEALTH.len(),
            HEALTH
        );
        health
            .write_all(health_response.as_bytes())
            .expect("write replacement health");

        let (mut mutation, _) = replacement
            .accept()
            .expect("accept mutation after replacement health");
        let mut mutation_request = String::new();
        mutation
            .read_to_string(&mut mutation_request)
            .expect("read negotiated mutation");
        mutation
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            .expect("acknowledge negotiated mutation");
        vec![first_request, health_request, mutation_request]
    });

    let error = client
        .post_empty(
            WriteEndpoint::SessionKill {
                session_id: "engine/42".to_owned(),
            },
            &serde_json::json!({}),
        )
        .expect_err("cached health must not authorize bytes to a replacement daemon");
    assert!(
        error.to_string().contains("daemon instance changed"),
        "unexpected replacement error: {error}"
    );

    client.health().expect("negotiate replacement daemon");
    client
        .post_empty(
            WriteEndpoint::SessionKill {
                session_id: "engine/42".to_owned(),
            },
            &serde_json::json!({}),
        )
        .expect("send only after negotiating the replacement");

    let requests = server.join().expect("replacement daemon thread");
    assert!(requests[0].is_empty(), "mutation leaked before negotiation");
    assert!(requests[1].starts_with("GET /api/v1/health HTTP/1.1\r\n"));
    assert!(requests[2].starts_with("POST /api/v1/sessions/engine/42/kill HTTP/1.1\r\n"));
}

#[cfg(unix)]
#[test]
fn replacement_capabilities_are_enforced_before_mutation_bytes() {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
    };

    let home = TestHome::new();
    let original = serve_once(&home.path, 200, HEALTH.to_owned());
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover original daemon");
    let mut client = DaemonClient::new(endpoint);
    client.health().expect("negotiate original daemon");
    original.join().expect("original daemon thread");

    let socket = home.path.join("coven.sock");
    fs::remove_file(&socket).expect("remove original daemon endpoint");
    let replacement = UnixListener::bind(&socket).expect("bind replacement daemon");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make replacement endpoint owner-only");
    let replacement_health = health_with_capabilities(serde_json::json!({
        "events": true,
        "eventCursor": "sequence",
        "structuredErrors": true
    }));
    let server = std::thread::spawn(move || {
        let (mut first, _) = replacement.accept().expect("accept peer check");
        let mut first_request = String::new();
        first
            .read_to_string(&mut first_request)
            .expect("read peer-check connection");
        if !first_request.is_empty() {
            first
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .expect("finish incorrectly transmitted mutation");
            return first_request;
        }

        let (mut health, _) = replacement.accept().expect("accept replacement health");
        let mut health_request = String::new();
        health
            .read_to_string(&mut health_request)
            .expect("read replacement health");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            replacement_health.len(),
            replacement_health
        );
        health
            .write_all(response.as_bytes())
            .expect("write replacement health");
        let (mut capability_check, _) = replacement
            .accept()
            .expect("accept replacement capability peer check");
        let mut capability_request = String::new();
        capability_check
            .read_to_string(&mut capability_request)
            .expect("read replacement capability peer check");
        if capability_request.is_empty() {
            first_request
        } else {
            capability_request
        }
    });

    client
        .post_empty(
            WriteEndpoint::SessionKill {
                session_id: "session-1".to_owned(),
            },
            &serde_json::json!({}),
        )
        .expect_err("replacement must first invalidate cached negotiation");
    client.health().expect("negotiate replacement capabilities");
    let error = client
        .post_empty(
            WriteEndpoint::SessionKill {
                session_id: "session-1".to_owned(),
            },
            &serde_json::json!({}),
        )
        .expect_err("missing replacement capability must block mutation");
    assert!(matches!(
        error,
        ClientError::CapabilityUnavailable {
            capability: "sessions"
        }
    ));
    assert!(
        server.join().expect("replacement daemon thread").is_empty(),
        "mutation leaked to replacement before capability enforcement"
    );
}

#[cfg(unix)]
static PROCESS_CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Computes `target` relative to `base` by walking up past the shared
/// prefix, without touching the filesystem.
#[cfg(unix)]
fn relative_path(base: &Path, target: &Path) -> PathBuf {
    let base_components: Vec<_> = base.components().collect();
    let target_components: Vec<_> = target.components().collect();
    let shared = base_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = PathBuf::new();
    for _ in shared..base_components.len() {
        relative.push("..");
    }
    for component in &target_components[shared..] {
        relative.push(component.as_os_str());
    }
    relative
}

/// Restores the process's working directory on drop, even if the guarded
/// test body panics, so a single failure cannot leave later tests running
/// against the wrong cwd.
#[cfg(unix)]
struct RestoreCwd {
    original: PathBuf,
    _guard: std::sync::MutexGuard<'static, ()>,
}

#[cfg(unix)]
impl Drop for RestoreCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

#[cfg(unix)]
#[test]
fn discovered_endpoint_is_stable_after_the_process_changes_directory() {
    // Discovery must not store a socket path that is only valid relative to
    // the cwd at discovery time. If it did, a caller changing directories
    // afterwards (e.g. a long-lived CLI process switching working trees)
    // would silently redirect requests to whatever now resolves at that
    // relative path -- potentially a different, unvalidated socket -- rather
    // than the owner-local socket that was actually checked above.
    //
    // This test mutates the process-wide cwd, which is unsafe to do
    // concurrently with other cwd-sensitive tests. Serialize via a
    // process-wide lock and always restore the original cwd through a
    // panic-safe guard; no other test in this binary depends on the cwd, so
    // this cannot make unrelated parallel tests flaky.
    let guard = PROCESS_CWD_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let original_cwd = std::env::current_dir().expect("read current directory");
    let _restore = RestoreCwd {
        original: original_cwd.clone(),
        _guard: guard,
    };

    // Reuse `TestHome`'s collision-safe unique directory allocation (its own
    // pid + atomic counter) instead of inventing a second counter here,
    // which previously raced with concurrently running tests for the same
    // path.
    let home = TestHome::new();
    let relative_home = relative_path(&original_cwd, &home.path);

    let server = serve_once(&home.path, 200, HEALTH.to_string());
    // Discover with a *relative* coven_home: the resulting endpoint must
    // resolve to an absolute socket path immediately rather than one that
    // stays relative to the cwd at this moment.
    let endpoint =
        DaemonEndpoint::discover(&relative_home).expect("discover with a relative coven_home");

    // Move away from the coven_home's parent directory before issuing any
    // request. If the endpoint still held a relative path, this request
    // would resolve through the *new* cwd instead of the socket that was
    // actually validated by `discover`.
    std::env::set_current_dir("/").expect("change the process working directory");

    let mut client = DaemonClient::new(endpoint);
    let health = client
        .health()
        .expect("connect through the pre-resolved absolute socket path after a cwd change");
    assert_eq!(health.api_version, PROTOCOL_VERSION);

    server.join().expect("server thread");
}

#[cfg(unix)]
#[test]
fn symlinked_ancestor_parent_components_connect_to_the_validated_socket() {
    use std::{
        io::{Read, Write},
        os::unix::{
            fs::{symlink, PermissionsExt},
            net::UnixListener,
        },
        sync::{atomic::AtomicBool, Arc},
        time::{Duration, Instant},
    };

    fn serve_until(
        listener: UnixListener,
        body: String,
        stop: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<bool> {
        listener
            .set_nonblocking(true)
            .expect("configure nonblocking test listener");
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if stop.load(Ordering::Acquire) {
                    return false;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = String::new();
                        stream
                            .read_to_string(&mut request)
                            .expect("read client request");
                        let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                        stream
                            .write_all(response.as_bytes())
                            .expect("write daemon response");
                        return true;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("accept client request: {error}"),
                }
            }
        })
    }

    let sandbox = TestHome::new();
    let target_parent = sandbox.path.join("t");
    let symlink_target = target_parent.join("p");
    let validated_home = target_parent.join("h");
    let lexically_retargeted_home = sandbox.path.join("h");
    fs::create_dir_all(&symlink_target).expect("create symlink target");
    fs::create_dir_all(&validated_home).expect("create validated Coven home");
    fs::create_dir_all(&lexically_retargeted_home).expect("create retargeted Coven home");
    fs::set_permissions(&validated_home, fs::Permissions::from_mode(0o700))
        .expect("make validated Coven home private");
    fs::set_permissions(
        &lexically_retargeted_home,
        fs::Permissions::from_mode(0o700),
    )
    .expect("make retargeted Coven home private");
    let linked_ancestor = sandbox.path.join("l");
    symlink(&symlink_target, &linked_ancestor).expect("create symlinked ancestor");

    let validated_socket = validated_home.join("coven.sock");
    let validated_listener =
        UnixListener::bind(&validated_socket).expect("bind validated daemon socket");
    fs::set_permissions(&validated_socket, fs::Permissions::from_mode(0o600))
        .expect("make validated daemon socket owner-only");

    let retargeted_socket = lexically_retargeted_home.join("coven.sock");
    let retargeted_listener =
        UnixListener::bind(&retargeted_socket).expect("bind retargeted daemon socket");
    fs::set_permissions(&retargeted_socket, fs::Permissions::from_mode(0o600))
        .expect("make retargeted daemon socket owner-only");

    let stop_servers = Arc::new(AtomicBool::new(false));
    let validated_server = serve_until(
        validated_listener,
        HEALTH.to_owned(),
        Arc::clone(&stop_servers),
    );
    let retargeted_server = serve_until(
        retargeted_listener,
        HEALTH.replace(PROTOCOL_VERSION, "coven.daemon.v2"),
        Arc::clone(&stop_servers),
    );

    let discovered_home = linked_ancestor.join("..").join("h");
    let endpoint = DaemonEndpoint::discover(&discovered_home)
        .expect("discover through a symlinked ancestor followed by parent traversal");
    let mut client = DaemonClient::new(endpoint);
    let health = client.health();
    stop_servers.store(true, Ordering::Release);

    let validated_was_connected = validated_server.join().expect("validated server thread");
    let retargeted_was_connected = retargeted_server.join().expect("retargeted server thread");
    let health = health.expect("connect to the socket whose target was validated");
    assert_eq!(health.api_version, PROTOCOL_VERSION);
    assert!(validated_was_connected);
    assert!(!retargeted_was_connected);
}

#[cfg(unix)]
#[test]
fn negotiates_before_an_empty_dependent_response() {
    let home = TestHome::new();
    let server = serve_responses(
        &home.path,
        vec![
            ("/api/v1/health".to_owned(), 200, HEALTH.to_string()),
            (
                "/api/v1/sessions/session-1/kill".to_owned(),
                204,
                String::new(),
            ),
        ],
    );
    let endpoint = DaemonEndpoint::discover(&home.path).expect("discover owner-local socket");
    let mut client = DaemonClient::new(endpoint);

    client
        .post_empty(
            WriteEndpoint::SessionKill {
                session_id: "session-1".to_owned(),
            },
            &serde_json::json!({}),
        )
        .expect("accept empty success response");

    server.join().expect("server thread");
}

fn lifecycle_status(home: &Path) -> LifecycleDaemonStatus {
    LifecycleDaemonStatus {
        pid: std::process::id(),
        started_at: "2026-08-16T12:00:00Z".to_owned(),
        socket: home.join("coven.sock").to_string_lossy().into_owned(),
    }
}

fn lifecycle_health(status: &LifecycleDaemonStatus) -> String {
    serde_json::json!({
        "ok": true,
        "apiVersion": PROTOCOL_VERSION,
        "covenVersion": "0.1.0",
        "capabilities": {
            "sessions": true,
            "events": true,
            "eventCursor": "sequence",
            "structuredErrors": true
        },
        "daemon": status,
    })
    .to_string()
}

#[test]
fn lifecycle_probe_returns_the_profile_bound_daemon_identity() {
    use std::time::Duration;

    let home = TestHome::new();
    let expected = lifecycle_status(&home.path);
    let server = serve_responses(
        &home.path,
        vec![("/health".to_owned(), 200, lifecycle_health(&expected))],
    );

    let actual = probe_unix_daemon_health(&home.path, Duration::from_secs(1))
        .expect("probe owner-local daemon")
        .expect("health included daemon identity");

    assert_eq!(actual, expected);
    server.join().expect("server thread");
}

#[test]
fn lifecycle_probe_rejects_a_forged_pid_from_the_connected_peer() {
    use std::time::Duration;

    let home = TestHome::new();
    let mut forged = lifecycle_status(&home.path);
    forged.pid = if std::process::id() == u32::MAX {
        u32::MAX - 1
    } else {
        std::process::id() + 1
    };
    let server = serve_responses(
        &home.path,
        vec![("/health".to_owned(), 200, lifecycle_health(&forged))],
    );

    let error = probe_unix_daemon_health(&home.path, Duration::from_secs(1))
        .expect_err("health cannot claim a PID other than its connected Unix peer");

    assert!(matches!(
        error,
        ClientError::Discovery(message)
            if message
                == format!(
                    "daemon health reported pid {} but the connected peer pid was {}",
                    forged.pid,
                    std::process::id()
                )
    ));
    server.join().expect("server thread");
}

#[test]
fn lifecycle_probe_accepts_and_canonicalizes_a_relative_same_profile_socket() {
    use std::time::Duration;

    let home = TestHome::new();
    let canonical_socket = fs::canonicalize(home.path.join("coven.sock"))
        .unwrap_or_else(|_| home.path.join("coven.sock"));
    let current_dir = std::env::current_dir().expect("current directory");
    let relative_socket = relative_path(&current_dir, &home.path.join("coven.sock"));
    let mut reported = lifecycle_status(&home.path);
    reported.socket = relative_socket.to_string_lossy().into_owned();
    let server = serve_responses(
        &home.path,
        vec![("/health".to_owned(), 200, lifecycle_health(&reported))],
    );

    let actual = probe_unix_daemon_health(&home.path, Duration::from_secs(1))
        .expect("probe same-profile relative socket")
        .expect("health included daemon identity");

    assert_eq!(Path::new(&actual.socket), canonical_socket);
    server.join().expect("server thread");
}

#[test]
fn lifecycle_probe_rejects_an_existing_cross_profile_socket() {
    use std::{
        os::unix::{fs::PermissionsExt, net::UnixListener},
        time::Duration,
    };

    let profile_a = TestHome::new();
    let profile_b = TestHome::new();
    let socket_b = profile_b.path.join("coven.sock");
    let _listener_b = UnixListener::bind(&socket_b).expect("bind other-profile socket");
    fs::set_permissions(&socket_b, fs::Permissions::from_mode(0o600))
        .expect("protect other-profile socket");
    let mut redirected = lifecycle_status(&profile_a.path);
    redirected.socket = socket_b.to_string_lossy().into_owned();
    let server = serve_responses(
        &profile_a.path,
        vec![("/health".to_owned(), 200, lifecycle_health(&redirected))],
    );

    let error = probe_unix_daemon_health(&profile_a.path, Duration::from_secs(1))
        .expect_err("reject a health identity for another profile");

    assert!(matches!(error, ClientError::Discovery(_)));
    server.join().expect("server thread");
}

#[test]
fn lifecycle_probe_uses_one_absolute_deadline_for_a_stalled_response() {
    use std::{
        io::Read,
        os::unix::{fs::PermissionsExt, net::UnixListener},
        sync::mpsc,
        time::Duration,
    };

    let home = TestHome::new();
    let socket = home.path.join("coven.sock");
    let listener = UnixListener::bind(&socket).expect("bind lifecycle socket");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make lifecycle socket private");
    let (accepted_tx, accepted_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept lifecycle probe");
        let mut request = String::new();
        stream.read_to_string(&mut request).expect("read probe");
        accepted_tx.send(()).expect("report accepted probe");
        release_rx.recv().expect("release stalled probe");
    });
    let home_path = home.path.clone();
    let (result_tx, result_rx) = mpsc::sync_channel(0);
    let client = std::thread::spawn(move || {
        result_tx
            .send(probe_unix_daemon_health(
                &home_path,
                Duration::from_millis(30),
            ))
            .expect("return probe result");
    });

    accepted_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("probe connected");
    let result = result_rx
        .recv_timeout(Duration::from_millis(250))
        .expect("stalled probe respected its deadline");
    assert!(result.is_err(), "stalled probe unexpectedly succeeded");
    release_tx.send(()).expect("release server");
    client.join().expect("client thread");
    server.join().expect("server thread");
}

#[test]
fn lifecycle_probe_rejects_trickling_and_unbounded_responses_quickly() {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        time::Duration,
    };

    let status = LifecycleDaemonStatus {
        pid: 42,
        started_at: "2026-08-16T12:00:00Z".to_owned(),
        socket: String::new(),
    };
    let health = lifecycle_health(&status);
    let valid = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{health}",
        health.len()
    )
    .into_bytes();
    let oversized_body = b"HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n".to_vec();
    let oversized_headers =
        format!("HTTP/1.1 200 OK\r\nX-Fill: {}\r\n", "x".repeat(70 * 1024)).into_bytes();

    for (response, trickle) in [
        (valid, true),
        (oversized_body, false),
        (oversized_headers, false),
    ] {
        let home = TestHome::new();
        let socket = home.path.join("coven.sock");
        let listener = UnixListener::bind(&socket).expect("bind lifecycle socket");
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
            .expect("make lifecycle socket private");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept lifecycle probe");
            let mut request = String::new();
            stream.read_to_string(&mut request).expect("read probe");
            if trickle {
                for byte in response {
                    if stream.write_all(&[byte]).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            } else {
                let _ = stream.write_all(&response);
            }
        });

        let started = std::time::Instant::now();
        let result = probe_unix_daemon_health(&home.path, Duration::from_millis(30));
        assert!(result.is_err(), "malformed unbounded response was accepted");
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "response deadline was reset by trickling bytes"
        );
        server.join().expect("server thread");
    }
}

#[test]
fn lifecycle_shutdown_waits_for_eof_on_the_authenticated_connection() {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        sync::mpsc,
        time::Duration,
    };

    let home = TestHome::new();
    let expected = lifecycle_status(&home.path);
    let socket = home.path.join("coven.sock");
    let listener = UnixListener::bind(&socket).expect("bind lifecycle socket");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make lifecycle socket private");
    let acknowledged = serde_json::json!({
        "ok": true,
        "apiVersion": PROTOCOL_VERSION,
        "capabilities": { "structuredErrors": true },
        "daemon": expected,
    })
    .to_string();
    let (ack_tx, ack_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept shutdown");
        let mut request = String::new();
        stream.read_to_string(&mut request).expect("read shutdown");
        assert!(request.starts_with("POST /api/v1/internal/lifecycle/shutdown HTTP/1.1\r\n"));
        let response = format!(
            "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            acknowledged.len(),
            acknowledged
        );
        stream
            .write_all(response.as_bytes())
            .expect("acknowledge shutdown");
        ack_tx.send(()).expect("report acknowledgement");
        release_rx.recv().expect("release shutdown connection");
    });
    let expected_for_client = lifecycle_status(&home.path);
    let home_path = home.path.clone();
    let (result_tx, result_rx) = mpsc::sync_channel(0);
    let client = std::thread::spawn(move || {
        result_tx
            .send(shutdown_unix_daemon(
                &home_path,
                &expected_for_client,
                Duration::from_secs(1),
            ))
            .expect("return shutdown result");
    });

    ack_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("shutdown was acknowledged");
    assert!(
        result_rx.recv_timeout(Duration::from_millis(30)).is_err(),
        "client returned before its authenticated connection closed"
    );
    release_tx.send(()).expect("close authenticated connection");
    assert_eq!(
        result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown result")
            .expect("valid shutdown response"),
        UnixDaemonShutdown::Exited
    );
    client.join().expect("client thread");
    server.join().expect("server thread");
}

#[test]
fn lifecycle_shutdown_rejects_a_buffered_post_response_byte() {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        time::Duration,
    };

    let home = TestHome::new();
    let expected = lifecycle_status(&home.path);
    let socket = home.path.join("coven.sock");
    let listener = UnixListener::bind(&socket).expect("bind lifecycle socket");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make lifecycle socket private");
    let acknowledged = serde_json::json!({
        "ok": true,
        "apiVersion": PROTOCOL_VERSION,
        "capabilities": { "structuredErrors": true },
        "daemon": expected,
    })
    .to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept shutdown");
        let mut request = String::new();
        stream.read_to_string(&mut request).expect("read shutdown");
        assert!(request.starts_with("POST /api/v1/internal/lifecycle/shutdown HTTP/1.1\r\n"));
        let response = format!(
            "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}X",
            acknowledged.len(),
            acknowledged
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response and trailing byte together");
    });

    let error = shutdown_unix_daemon(
        &home.path,
        &lifecycle_status(&home.path),
        Duration::from_secs(1),
    )
    .expect_err("reject a byte after the retained lifecycle response");

    assert!(matches!(
        error,
        ClientError::InvalidHttpResponse(message)
            if message == "daemon sent bytes after its lifecycle response completed"
    ));
    server.join().expect("server thread");
}

#[test]
fn lifecycle_shutdown_validates_a_malformed_404_before_base_fallback() {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };

    let home = TestHome::new();
    let mut expected = lifecycle_status(&home.path);
    expected.pid = std::process::id();
    let socket = home.path.join("coven.sock");
    let listener = UnixListener::bind(&socket).expect("bind lifecycle socket");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make lifecycle socket private");
    let fallback_observed = Arc::new(AtomicBool::new(false));
    let fallback_for_server = Arc::clone(&fallback_observed);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept shutdown");
        let mut request = String::new();
        stream.read_to_string(&mut request).expect("read shutdown");
        assert!(request.starts_with("POST /api/v1/internal/lifecycle/shutdown HTTP/1.1\r\n"));
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\nX")
            .expect("write malformed missing-route response");
        drop(stream);

        listener
            .set_nonblocking(true)
            .expect("make fallback detection nonblocking");
        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut fallback, _)) => {
                    fallback_for_server.store(true, Ordering::Release);
                    let mut request = String::new();
                    fallback
                        .read_to_string(&mut request)
                        .expect("read fallback request");
                    fallback
                        .write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 0\r\n\r\n")
                        .expect("reject unexpected fallback");
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("failed checking for fallback request: {error}"),
            }
        }
    });

    let error = shutdown_unix_daemon(&home.path, &expected, Duration::from_secs(1))
        .expect_err("reject malformed 404 framing before fallback");

    assert!(matches!(
        error,
        ClientError::InvalidHttpResponse(message) if message.contains("bytes after")
    ));
    server.join().expect("server thread");
    assert!(
        !fallback_observed.load(Ordering::Acquire),
        "malformed 404 triggered BASE fallback"
    );
}

#[test]
fn lifecycle_shutdown_never_uses_legacy_fallback_for_non_404_errors() {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        time::{Duration, Instant},
    };

    for status in [400, 401, 403, 405, 429, 500, 503] {
        let home = TestHome::new();
        let expected = lifecycle_status(&home.path);
        let socket = home.path.join("coven.sock");
        let listener = UnixListener::bind(&socket).expect("bind lifecycle socket");
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
            .expect("make lifecycle socket private");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept shutdown");
            let mut request = String::new();
            stream.read_to_string(&mut request).expect("read shutdown");
            assert!(request.starts_with("POST /api/v1/internal/lifecycle/shutdown HTTP/1.1\r\n"));
            write!(
                stream,
                "HTTP/1.1 {status} Error\r\nContent-Length: 0\r\n\r\n"
            )
            .expect("write rejected shutdown");
            drop(stream);

            listener
                .set_nonblocking(true)
                .expect("make fallback detection nonblocking");
            let deadline = Instant::now() + Duration::from_millis(100);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok(_) => panic!("HTTP {status} incorrectly triggered a legacy health request"),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("failed checking for a fallback request: {error}"),
                }
            }
        });

        let error = shutdown_unix_daemon(&home.path, &expected, Duration::from_secs(1))
            .expect_err("non-404 shutdown error must be returned");
        assert!(
            matches!(error, ClientError::HttpStatus(actual) if actual == status),
            "unexpected shutdown result for HTTP {status}: {error}"
        );
        server.join().expect("server thread");
    }
}

/// A daemon that accepts a shutdown request, writes its 202 acknowledgement,
/// and then unlinks its own socket path before closing the authenticated
/// connection is exercising a legitimate shutdown sequence: nothing after
/// the pre-send snapshot may re-resolve that pathname against the
/// filesystem, since it may no longer exist by the time validation runs.
/// The stop must still resolve to `Exited`, and the now-unlinked path must
/// be immediately reusable by a freshly started daemon.
#[test]
fn lifecycle_shutdown_succeeds_when_the_daemon_unlinks_its_socket_before_closing() {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        time::Duration,
    };

    let home = TestHome::new();
    let expected = lifecycle_status(&home.path);
    let socket = home.path.join("coven.sock");
    let listener = UnixListener::bind(&socket).expect("bind lifecycle socket");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make lifecycle socket private");
    let acknowledged = serde_json::json!({
        "ok": true,
        "apiVersion": PROTOCOL_VERSION,
        "capabilities": { "structuredErrors": true },
        "daemon": expected,
    })
    .to_string();
    let socket_for_server = socket.clone();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept shutdown");
        let mut request = String::new();
        stream.read_to_string(&mut request).expect("read shutdown");
        assert!(request.starts_with("POST /api/v1/internal/lifecycle/shutdown HTTP/1.1\r\n"));
        let response = format!(
            "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            acknowledged.len(),
            acknowledged
        );
        stream
            .write_all(response.as_bytes())
            .expect("acknowledge shutdown");
        // Legitimate daemon shutdown: unlink the socket path before the
        // authenticated connection is closed, exactly as a real daemon does
        // when it releases its listener ahead of process exit.
        fs::remove_file(&socket_for_server).expect("unlink daemon socket during shutdown");
        drop(stream);
    });

    let result = shutdown_unix_daemon(&home.path, &expected, Duration::from_secs(1))
        .expect("an unlinked-before-close shutdown must not be treated as an error");
    server.join().expect("server thread");

    assert_eq!(result, UnixDaemonShutdown::Exited);
    assert!(
        !socket.exists(),
        "the daemon's unlink of its own socket must not be undone by validation"
    );

    // Stop/restart succeeds: nothing left over from validating the shutdown
    // may block a freshly started daemon from binding the same path.
    let restarted = UnixListener::bind(&socket).expect("restart must reuse the unlinked path");
    drop(restarted);
}

/// If the socket path is rebound to a different daemon before the shutdown
/// request is authenticated, the connected peer's real (kernel-authenticated)
/// identity must still be checked against the caller's expectation, even if
/// a response body claims otherwise. The daemon-echoed acknowledgement body
/// alone is not sufficient: a numeric pid substitution is a hard failure,
/// not a soft identity mismatch, so it must fail closed rather than report a
/// successful stop or silently defer to a fallback path for a peer it never
/// actually expected to reach.
#[test]
fn lifecycle_shutdown_rejects_a_binding_replaced_before_the_request() {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        time::Duration,
    };

    let home = TestHome::new();
    let mut expected = lifecycle_status(&home.path);
    // The recorded identity no longer matches the real, kernel-authenticated
    // peer this connection will reach (which, over a Unix socket, is always
    // this test process' own pid): this models the path having been rebound
    // to a different daemon since `expected` was last recorded.
    expected.pid = if std::process::id() == u32::MAX {
        u32::MAX - 1
    } else {
        std::process::id() + 1
    };
    let socket = home.path.join("coven.sock");
    let listener = UnixListener::bind(&socket).expect("bind lifecycle socket");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make lifecycle socket private");
    let acknowledged = serde_json::json!({
        "ok": true,
        "apiVersion": PROTOCOL_VERSION,
        "capabilities": { "structuredErrors": true },
        "daemon": expected,
    })
    .to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept shutdown");
        let mut request = String::new();
        stream.read_to_string(&mut request).expect("read shutdown");
        assert!(request.starts_with("POST /api/v1/internal/lifecycle/shutdown HTTP/1.1\r\n"));
        // Even a (misbehaving or confused) peer that echoes back an
        // acknowledgement matching the client's claimed identity must not be
        // trusted over the authenticated connection's real peer identity.
        let response = format!(
            "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            acknowledged.len(),
            acknowledged
        );
        stream
            .write_all(response.as_bytes())
            .expect("acknowledge mismatched shutdown");
    });

    let error = shutdown_unix_daemon(&home.path, &expected, Duration::from_secs(1)).expect_err(
        "a substituted pid must not be authenticated as the expected daemon, even when the \
         response body claims otherwise",
    );

    assert!(
        matches!(&error, ClientError::Discovery(message) if message.contains("connected peer pid")),
        "unexpected rejection for a replaced binding: {error}"
    );
    server.join().expect("server thread");
}
