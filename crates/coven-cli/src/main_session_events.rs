//! Bounded, pull-driven SSE for the main conversation. No producer queue and
//! no SQLite snapshot or scope lock survives a network write.
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::json;

use crate::{
    api::{query_param, store_path},
    api_response::{api_error, ApiResponse},
    main_session::{self, MainSessionRecord, INSTANCE_MAIN_SCOPE_KEY},
    main_session_routes::ScopeGate,
    store,
};

pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const HEARTBEAT: Duration = Duration::from_secs(25);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_EVENT_BYTES: usize = 1024 * 1024;
const MAX_STREAMS: usize = 16;
static STREAMS: AtomicUsize = AtomicUsize::new(0);

struct Permit<'a>(&'a AtomicUsize);
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
fn acquire(counter: &AtomicUsize) -> Option<Permit<'_>> {
    let mut count = counter.load(Ordering::Acquire);
    while count < MAX_STREAMS {
        match counter.compare_exchange_weak(count, count + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(Permit(counter)),
            Err(current) => count = current,
        }
    }
    None
}

pub(crate) fn claims(method: &str, path: &str) -> bool {
    method == "GET"
        && matches!(
            crate::api_routes::normalize_api_route(crate::api_routes::split_path_query(path).0),
            crate::api_routes::ApiRoute::Route(route) if route == "/main-session/events"
        )
}

pub(crate) struct Stream {
    home: PathBuf,
    scope: String,
    conversation: String,
    harness: String,
    after: i64,
    reset_cursor: bool,
    last_write: Instant,
}

enum Next {
    Idle,
    Event { seq: i64, json: String },
    Changed(MainSessionRecord),
    TooLarge(i64),
    Gone,
}

impl Stream {
    pub(crate) fn open(
        home: &Path,
        query: Option<&str>,
        last_id: Option<&str>,
    ) -> Result<Result<Self, ApiResponse>> {
        let scope =
            query_param(query.unwrap_or_default(), "scope").unwrap_or(INSTANCE_MAIN_SCOPE_KEY);
        let after = match last_id.filter(|id| !id.is_empty()) {
            None => 0,
            Some(id) if id.bytes().all(|byte| byte.is_ascii_digit()) => match id.parse::<i64>() {
                Ok(seq) if seq > 0 => seq,
                _ => {
                    return Ok(Err(api_error(
                        400,
                        "invalid_request",
                        "Last-Event-ID must be a positive event seq.",
                        None,
                    )?))
                }
            },
            Some(_) => {
                return Ok(Err(api_error(
                    400,
                    "invalid_request",
                    "Last-Event-ID must be a positive event seq.",
                    None,
                )?))
            }
        };
        if let Err(error) = main_session::validate_scope_key(scope) {
            return Ok(Err(api_error(
                400,
                "invalid_request",
                &error.to_string(),
                None,
            )?));
        }
        let conn = store::open_store(&store_path(home))?;
        let Some(record) = main_session::get_main_session(&conn, scope)? else {
            return Ok(Err(api_error(
                404,
                "main_session_not_found",
                "No main session exists for this scope.",
                None,
            )?));
        };
        if after != 0
            && !conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM events WHERE rowid = ?1)",
                [after],
                |row| row.get::<_, bool>(0),
            )?
        {
            return Ok(Err(api_error(
                409,
                "event_cursor_unavailable",
                "The event cursor is unavailable; refresh history before resubscribing.",
                None,
            )?));
        }
        // A reset deliberately starts a new conversation. Never apply an old
        // conversation's cursor to the replacement (nor reveal its events).
        let reset_cursor = after != 0
            && !conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM events e JOIN sessions s ON s.id = e.session_id
             WHERE e.rowid = ?1 AND s.conversation_id = ?2 AND s.harness = ?3)",
                params![after, record.conversation_id, record.harness],
                |row| row.get::<_, bool>(0),
            )?;
        Ok(Ok(Self {
            home: home.to_owned(),
            scope: scope.to_owned(),
            conversation: record.conversation_id,
            harness: record.harness,
            after: if reset_cursor { 0 } else { after },
            reset_cursor,
            last_write: Instant::now(),
        }))
    }

    fn next(&self) -> Result<Next> {
        let Some(_gate) = ScopeGate::try_acquire(&self.home, &self.scope)? else {
            return Ok(Next::Idle);
        };
        let mut conn = store::open_store(&store_path(&self.home))?;
        let tx = conn.transaction()?;
        let Some(record) = main_session::get_main_session(&tx, &self.scope)? else {
            return Ok(Next::Gone);
        };
        if record.conversation_id != self.conversation || record.harness != self.harness {
            return Ok(Next::Changed(record));
        }
        // Only the next row's identity is loaded before checking payload size.
        // The scope gate prevents a launch's startup events from being bound
        // behind a cursor already advanced by the preceding session.
        let next: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT e.rowid, CASE WHEN octet_length(e.session_id) <= ?4 THEN e.session_id ELSE NULL END
             FROM events e JOIN sessions s ON s.id = e.session_id
             WHERE s.conversation_id = ?1 AND s.harness = ?2 AND e.rowid > ?3
             ORDER BY e.rowid LIMIT 1",
                params![self.conversation, self.harness, self.after, MAX_EVENT_BYTES as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((seq, session)) = next else {
            return Ok(Next::Idle);
        };
        let Some(session) = session else {
            return Ok(Next::TooLarge(seq));
        };
        let candidates = store::list_event_candidates(&tx, &session, Some(seq - 1), 1)?;
        let candidate = candidates
            .candidates
            .first()
            .context("stream event disappeared from snapshot")?;
        if candidate.allocation_bytes > MAX_EVENT_BYTES {
            return Ok(Next::TooLarge(seq));
        }
        let event = store::get_event_by_seq(&tx, &session, seq)?
            .context("stream event disappeared from snapshot")?;
        let Some(json) = crate::api::serialize_event_with_limit(&event, MAX_EVENT_BYTES)? else {
            return Ok(Next::TooLarge(seq));
        };
        Ok(Next::Event { seq, json })
    }

    /// Returns whether another immediate read may be useful. At most one
    /// bounded event is resident, even while the socket writer is blocked.
    fn tick(&mut self, write: &mut impl Write, now: Instant) -> Result<(bool, bool)> {
        if self.reset_cursor {
            write_frame(write, b"id:\nevent: main_session.changed\ndata: {}\n\n")?;
            self.reset_cursor = false;
            self.last_write = now;
            return Ok((true, true));
        }
        let next = self.next()?;
        match next {
            Next::Event { seq, json } => {
                write_frame(write, format!("id: {seq}\ndata: {json}\n\n").as_bytes())?;
                self.after = seq;
            }
            Next::Changed(record) => {
                self.conversation = record.conversation_id;
                self.harness = record.harness;
                self.after = 0;
                self.reset_cursor = true;
                return Ok((true, true));
            }
            Next::TooLarge(seq) => {
                let data =
                    json!({"error":{"code":"event_response_too_large","details":{"seq":seq}}});
                write_frame(write, format!("event: error\ndata: {data}\n\n").as_bytes())?;
                return Ok((false, false));
            }
            Next::Gone => return Ok((false, false)),
            Next::Idle => {
                if now.duration_since(self.last_write) >= HEARTBEAT {
                    write_frame(write, b": heartbeat\n\n")?;
                    self.last_write = now;
                }
                return Ok((true, false));
            }
        }
        self.last_write = now;
        Ok((true, true))
    }

    pub(crate) fn serve(mut self, write: &mut impl Write) -> Result<()> {
        write_frame(write, b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nX-Accel-Buffering: no\r\nConnection: close\r\n\r\n: connected\n\n")?;
        while !crate::daemon::daemon_termination_requested() {
            let (keep, ready) = self.tick(write, Instant::now())?;
            if !keep {
                break;
            }
            if !ready {
                std::thread::sleep(POLL_INTERVAL);
            }
        }
        Ok(())
    }
}

/// The transport must install WRITE_TIMEOUT before calling this. The deadline
/// also stops a reader making tiny partial progress from retaining a frame
/// forever. A blocking OS write can overshoot by at most one socket timeout.
fn write_frame(write: &mut impl Write, mut frame: &[u8]) -> Result<()> {
    let deadline = Instant::now() + WRITE_TIMEOUT;
    while !frame.is_empty() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "SSE frame write deadline exceeded",
            )
            .into());
        }
        match write.write(frame) {
            // PIPE_NOWAIT can report zero bytes when its buffer is full.
            Ok(0) => std::thread::sleep(Duration::from_millis(5)),
            Ok(count) => frame = &frame[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error.into()),
        }
    }
    write.flush().context("failed to flush SSE frame")
}

pub(crate) fn handle(
    home: &Path,
    query: Option<&str>,
    last_id: Option<&str>,
    write: &mut impl Write,
    configure: Option<&dyn Fn() -> Result<()>>,
) -> Result<Option<ApiResponse>> {
    let stream = match Stream::open(home, query, last_id)? {
        Ok(stream) => stream,
        Err(response) => return Ok(Some(response)),
    };
    let Some(_permit) = acquire(&STREAMS) else {
        return api_error(
            503,
            "stream_capacity",
            "Too many active event streams; retry later.",
            None,
        )
        .map(Some);
    };
    configure.context("transport does not provide bounded SSE writes")?()?;
    stream.serve(write)?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    const NOW: &str = "2026-10-15T12:00:00Z";

    struct Home {
        dir: tempfile::TempDir,
        conn: rusqlite::Connection,
        record: MainSessionRecord,
    }
    impl Home {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            crate::daemon::ensure_private_coven_home(dir.path()).unwrap();
            let mut conn = store::open_store(&store_path(dir.path())).unwrap();
            let record = main_session::resolve_main_session(
                &mut conn,
                INSTANCE_MAIN_SCOPE_KEY,
                None,
                "claude",
                "/tmp/project",
                NOW,
            )
            .unwrap()
            .record;
            let home = Self { dir, conn, record };
            home.session("first", &home.record.conversation_id);
            home
        }
        fn session(&self, id: &str, conversation: &str) {
            self.conn
                .execute(
                    "INSERT INTO sessions
                (id, project_root, harness, title, status, created_at, updated_at, conversation_id)
                VALUES (?1, '/tmp/project', 'claude', 'Home', 'running', ?2, ?2, ?3)",
                    params![id, NOW, conversation],
                )
                .unwrap();
        }
        fn event(&self, session: &str, payload: &str) -> i64 {
            store::insert_event(
                &self.conn,
                &store::EventRecord {
                    seq: 0,
                    id: uuid::Uuid::new_v4().to_string(),
                    session_id: session.to_owned(),
                    kind: "output".to_owned(),
                    payload_json: payload.to_owned(),
                    created_at: NOW.to_owned(),
                },
            )
            .unwrap();
            store::latest_event_seq(&self.conn, session).unwrap()
        }
        fn stream(&self, after: Option<i64>) -> Stream {
            Stream::open(
                self.dir.path(),
                None,
                after.map(|seq| seq.to_string()).as_deref(),
            )
            .unwrap()
            .unwrap()
        }
    }
    fn ids(bytes: &[u8]) -> Vec<i64> {
        std::str::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter_map(|line| line.strip_prefix("id: "))
            .map(|id| id.parse().unwrap())
            .collect()
    }
    fn drain(stream: &mut Stream, out: &mut impl Write) {
        for _ in 0..100 {
            let (keep, ready) = stream.tick(out, Instant::now()).unwrap();
            if !keep || !ready {
                return;
            }
        }
        panic!("stream did not drain");
    }

    #[test]
    fn reconnect_replays_without_gaps_duplicates_or_unrelated_sessions() {
        let home = Home::new();
        let first = home.event("first", r#"{"text":"first\nline"}"#);
        home.session("unrelated", "other-conversation");
        home.event("unrelated", r#"{"text":"private-other-conversation"}"#);
        let second = home.event("first", r#"{"text":"second"}"#);
        let mut stream = home.stream(None);
        let mut before_disconnect = Vec::new();
        stream.tick(&mut before_disconnect, Instant::now()).unwrap();
        assert_eq!(ids(&before_disconnect), [first]);
        drop(stream);
        home.session("resumed", &home.record.conversation_id);
        let third = home.event("resumed", r#"{"text":"third"}"#);
        let mut resumed = home.stream(Some(first));
        let mut after_reconnect = Vec::new();
        drain(&mut resumed, &mut after_reconnect);
        assert_eq!(ids(&after_reconnect), [second, third]);
        assert!(!String::from_utf8(after_reconnect)
            .unwrap()
            .contains("private-other"));
    }

    #[test]
    fn launch_gate_prevents_cursor_advancing_past_unbound_startup_events() {
        let home = Home::new();
        let mut stream = home.stream(None);
        let gate = ScopeGate::acquire(home.dir.path(), INSTANCE_MAIN_SCOPE_KEY).unwrap();
        home.session("starting", "provisional-conversation");
        let startup = home.event("starting", r#"{"text":"startup"}"#);
        let previous_tail = home.event("first", r#"{"text":"previous tail"}"#);
        let mut out = Vec::new();
        assert_eq!(
            stream.tick(&mut out, Instant::now()).unwrap(),
            (true, false)
        );
        assert!(out.is_empty());
        home.conn
            .execute(
                "UPDATE sessions SET conversation_id = ?1 WHERE id = 'starting'",
                [&home.record.conversation_id],
            )
            .unwrap();
        drop(gate);
        drain(&mut stream, &mut out);
        assert_eq!(ids(&out), [startup, previous_tail]);
    }

    #[test]
    fn reset_clears_cursor_and_follows_replacement_without_old_conversation_output() {
        let mut home = Home::new();
        let old = home.event("first", "{}");
        let mut live = home.stream(Some(old));
        main_session::reset_main_session(
            &mut home.conn,
            INSTANCE_MAIN_SCOPE_KEY,
            &main_session::MainSessionSettings::default(),
            NOW,
        )
        .unwrap();
        let record = main_session::get_main_session(&home.conn, INSTANCE_MAIN_SCOPE_KEY)
            .unwrap()
            .unwrap();
        home.session("replacement", &record.conversation_id);
        let new = home.event("replacement", r#"{"text":"new conversation"}"#);
        home.event("first", r#"{"text":"late old conversation"}"#);
        for stream in [&mut live, &mut home.stream(Some(old))] {
            let mut out = Vec::new();
            drain(stream, &mut out);
            assert_eq!(ids(&out), [new]);
            let text = String::from_utf8(out).unwrap();
            assert!(text.starts_with("id:\nevent: main_session.changed\n"));
            assert!(!text.contains("late old"));
        }
    }

    #[test]
    fn heartbeat_is_a_comment_and_never_moves_the_cursor() {
        let home = Home::new();
        let mut stream = home.stream(None);
        let mut out = Vec::new();
        let start = stream.last_write;
        stream
            .tick(&mut out, start + HEARTBEAT - Duration::from_nanos(1))
            .unwrap();
        assert!(out.is_empty());
        stream.tick(&mut out, start + HEARTBEAT).unwrap();
        assert_eq!(out, b": heartbeat\n\n");
        assert_eq!(stream.after, 0);
    }

    #[test]
    fn oversized_row_is_not_loaded_or_skipped() {
        let home = Home::new();
        let first = home.event("first", "{}");
        home.conn
            .execute(
                "UPDATE events SET payload_json = printf('%.*c', ?1, 'x') WHERE rowid = ?2",
                params![(MAX_EVENT_BYTES + 1) as i64, first],
            )
            .unwrap();
        home.event("first", "{}");
        let mut stream = home.stream(None);
        assert!(matches!(stream.next().unwrap(), Next::TooLarge(seq) if seq == first));
        let mut out = Vec::new();
        assert_eq!(
            stream.tick(&mut out, Instant::now()).unwrap(),
            (false, false)
        );
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("event_response_too_large"));
        assert_eq!(stream.after, 0);
    }

    #[test]
    fn oversized_session_identifier_is_rejected_before_loading_it() {
        let home = Home::new();
        home.conn.execute("INSERT INTO sessions
            (id, project_root, harness, title, status, created_at, updated_at, conversation_id)
            VALUES (printf('%.*c', ?1, 's'), '/tmp/project', 'claude', 'Home', 'completed', ?2, ?2, ?3)",
            params![(MAX_EVENT_BYTES + 1) as i64, NOW, home.record.conversation_id]).unwrap();
        home.conn.execute("INSERT INTO events (id, session_id, kind, payload_json, created_at)
            SELECT 'large-session-event', id, 'output', '{}', ?1 FROM sessions WHERE length(id) > ?2",
            params![NOW, MAX_EVENT_BYTES as i64]).unwrap();
        assert!(matches!(
            home.stream(None).next().unwrap(),
            Next::TooLarge(_)
        ));
    }

    #[test]
    fn legacy_payload_is_redacted_and_frame_data_cannot_inject_sse_fields() {
        let home = Home::new();
        let seq = home.event("first", "{}");
        home.conn.execute("UPDATE events SET payload_json = ?1, redaction_status = 'legacy' WHERE rowid = ?2",
            params!["{\"password\":\"do-not-echo-this-value\",\"text\":\"line\\nid: 999\\ndata: forged\"}", seq]).unwrap();
        let mut out = Vec::new();
        drain(&mut home.stream(None), &mut out);
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("do-not-echo-this-value"));
        assert_eq!(
            text.lines().filter(|line| line.starts_with("id:")).count(),
            1
        );
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("data:"))
                .count(),
            1
        );
    }

    #[test]
    fn invalid_or_missing_cursor_fails_before_stream_headers() {
        let home = Home::new();
        for bad in ["-1", "abc", "1,2", "9223372036854775808", "0"] {
            let response = Stream::open(home.dir.path(), None, Some(bad))
                .unwrap()
                .err()
                .unwrap();
            assert_eq!(response.status, 400, "{bad}");
        }
        let response = Stream::open(home.dir.path(), None, Some("999"))
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(response.status, 409);
        let response = Stream::open(home.dir.path(), Some("scope=missing"), None)
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(response.status, 404);
    }

    #[test]
    fn stream_capacity_is_bounded_and_released_on_drop() {
        let counter = AtomicUsize::new(0);
        let permits: Vec<_> = (0..MAX_STREAMS)
            .map(|_| acquire(&counter).unwrap())
            .collect();
        assert!(acquire(&counter).is_none());
        drop(permits);
        assert!(acquire(&counter).is_some());
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    fn blocked_writer_holds_one_frame_and_no_database_or_scope_lock() {
        use std::sync::mpsc;
        let home = Home::new();
        let first = home.event("first", "{}");
        let mut stream = home.stream(None);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        struct BlockedWriter {
            entered: mpsc::Sender<usize>,
            release: mpsc::Receiver<()>,
            calls: usize,
        }
        impl Write for BlockedWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.calls += 1;
                self.entered.send(bytes.len()).unwrap();
                self.release.recv().unwrap();
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "injected slow reader",
                ))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let worker = std::thread::spawn(move || {
            let mut writer = BlockedWriter {
                entered: entered_tx,
                release: release_rx,
                calls: 0,
            };
            let result = stream.tick(&mut writer, Instant::now());
            (result, stream.after, writer.calls)
        });
        let held_bytes = entered_rx.recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(held_bytes <= MAX_EVENT_BYTES + 64);
        // These writes and this lock acquisition must succeed while network I/O
        // is paused. Increase the durable backlog without growing that frame.
        let gate = ScopeGate::try_acquire(home.dir.path(), INSTANCE_MAIN_SCOPE_KEY)
            .unwrap()
            .unwrap();
        for _ in 0..128 {
            home.event("first", "{}");
        }
        drop(gate);
        release_tx.send(()).unwrap();
        let (result, cursor, calls) = worker.join().unwrap();
        assert!(result.is_err());
        assert_eq!(cursor, 0, "failed frame {first} must be replayable");
        assert_eq!(calls, 1, "no read-ahead queue while the writer is blocked");
    }

    #[cfg(unix)]
    #[test]
    fn actual_slow_socket_reader_disconnects_with_bounded_frame() {
        use std::os::unix::net::UnixStream;
        let home = Home::new();
        // Use enough bounded records to saturate the OS socket buffer, without
        // allocating an application-side backlog on the streaming thread.
        let payload = serde_json::to_string(&"x".repeat(256 * 1024)).unwrap();
        for _ in 0..64 {
            home.event("first", &payload);
        }
        let (mut writer, _nonreading_client) = UnixStream::pair().unwrap();
        writer
            .set_write_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let mut stream = home.stream(None);
        let mut failure = None;
        for _ in 0..64 {
            if let Err(error) = stream.tick(&mut writer, Instant::now()) {
                failure = Some(error);
                break;
            }
        }
        let error = failure.expect("the nonreading socket must exert backpressure");
        let kind = error.downcast_ref::<io::Error>().unwrap().kind();
        assert!(
            matches!(kind, io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut),
            "{error:#}"
        );
        assert!(stream.after < store::latest_event_seq(&home.conn, "first").unwrap());
    }
}
