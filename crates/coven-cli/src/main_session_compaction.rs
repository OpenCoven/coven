//! Daemon-owned compaction of the main conversation (coven#1181, epic #1183).
//!
//! Claude compacts natively: `/compact <instructions>` on a stream process's
//! stdin summarises the conversation in place, then emits a `compact_boundary`
//! frame, the summary as a synthetic user frame, and a `result`. The daemon
//! decides *when*, so Home compacts on the daemon's terms, with fixed summary
//! sections, before the harness's own automatic point. Each compaction is a
//! row in `main_session_compactions`:
//!
//! 1. **Request.** On a scheduler tick, when Home's host process is idle and
//!    its reported fill ([`main_session_transcript`]) has reached
//!    [`THRESHOLD_PERCENT`] of the window, the daemon commits a `requested`
//!    row naming the host session and its newest event cursor, then sends
//!    `/compact` as a daemon input.
//! 2. **Settle.** Later ticks read the host's output after that cursor. The
//!    first `result` frame ends the command: with a `compact_boundary` before
//!    it the row settles `compacted` and a `main_session.compacted` event,
//!    carrying the token counts and the summary, lands on the host session and
//!    so in Home's event stream. Without one, a stopped host, or no result by
//!    the deadline, the row settles `failed` with a reason.
//!
//! The row is committed before the input is sent and settlement reads only
//! what the process emitted after the row's cursor, so a crash at any point
//! leaves a row the next tick settles from evidence. At most one compaction
//! per conversation is unsettled at a time.
//!
//! Compaction cannot shrink the system prompt and tools, so the first fill
//! reading after a compaction becomes its floor, and the next compaction waits
//! until the fill has grown [`REGROWTH_PERCENT`] of the window above it. A
//! large fixed baseline therefore cannot loop compactions. A failure holds off
//! retries for [`RETRY_AFTER_MINUTES`].
//!
//! Codex has no native compaction on its daemon path; its summary-and-rotation
//! slice is separate, so only Claude main sessions are compacted here.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    api::{store_path, SessionRuntime},
    main_session::{self, MainSessionRecord},
    main_session_routes::ScopeGate,
    main_session_transcript::{self, OutputLines},
    store,
};

pub const MAIN_SESSION_COMPACTIONS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS main_session_compactions (
        id TEXT PRIMARY KEY NOT NULL,
        scope_key TEXT NOT NULL,
        conversation_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        after_seq INTEGER NOT NULL,
        fill_tokens INTEGER NOT NULL,
        window_tokens INTEGER NOT NULL,
        state TEXT NOT NULL CHECK (state IN ('requested', 'compacted', 'failed')),
        requested_at TEXT NOT NULL,
        settled_at TEXT,
        boundary_seq INTEGER,
        pre_tokens INTEGER,
        post_tokens INTEGER,
        floor_tokens INTEGER,
        reason TEXT,
        CHECK ((state = 'requested') = (settled_at IS NULL)),
        CHECK ((state = 'compacted') = (boundary_seq IS NOT NULL)),
        CHECK ((state = 'failed') = (reason IS NOT NULL))
    );

    CREATE UNIQUE INDEX IF NOT EXISTS idx_main_session_compactions_open
        ON main_session_compactions(conversation_id) WHERE state = 'requested';
    CREATE INDEX IF NOT EXISTS idx_main_session_compactions_conversation
        ON main_session_compactions(conversation_id, requested_at);
";

const EVENT_KIND_COMPACTED: &str = "main_session.compacted";
/// Fill, as a percentage of the reported window, at which Home compacts. It
/// sits below Claude's automatic point for its supported windows.
pub const THRESHOLD_PERCENT: u64 = 60;
/// Growth above a compaction's floor, as a percentage of the window, before
/// the next compaction.
pub const REGROWTH_PERCENT: u64 = 10;
pub const RETRY_AFTER_MINUTES: i64 = 30;
/// How long a requested compaction may take to produce its result.
const DEADLINE_MINUTES: i64 = 10;
const SCAN_MAX_EVENTS: usize = 512;
const SCAN_MAX_BYTES: u64 = 4 * 1024 * 1024;
const SUMMARY_MAX_BYTES: usize = 16 * 1024;
const REASON_MAX_BYTES: usize = 1024;

/// Follows `/compact`. Claude adds these to its own summary instructions.
pub const COMPACT_INSTRUCTIONS: &str = "Coven is compacting Home, the long-running main \
     conversation. Write the summary under exactly these headings, in this order: Decisions, \
     Open TODOs, Constraints, Pending user asks, Exact identifiers. Under Exact identifiers, copy \
     verbatim every id, path, URL, name, number and codeword the conversation still depends on.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionRecord {
    pub id: String,
    pub conversation_id: String,
    pub session_id: String,
    pub state: String,
    pub fill_tokens: i64,
    pub window_tokens: i64,
    pub requested_at: String,
    pub settled_at: Option<String>,
    pub pre_tokens: Option<i64>,
    pub post_tokens: Option<i64>,
    pub reason: Option<String>,
}

/// Compactions a tick settled `failed`, with reasons, for the daemon's
/// recovery log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CompactionReport {
    pub failed: Vec<(String, String)>,
}

/// Settles open compactions and requests new ones, for every Claude main
/// session whose scope gate is free. A busy scope waits for the next tick.
pub fn tick(
    coven_home: &Path,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
) -> Result<CompactionReport> {
    let scopes: Vec<String> = {
        let conn = store::open_store(&store_path(coven_home))?;
        let mut statement =
            conn.prepare("SELECT scope_key FROM main_sessions WHERE harness = 'claude'")?;
        let scopes = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()
            .context("failed to list main sessions for compaction")?;
        scopes
    };
    let mut report = CompactionReport::default();
    for scope in scopes {
        let Some(_gate) = ScopeGate::try_acquire(coven_home, &scope)? else {
            continue;
        };
        let conn = store::open_store(&store_path(coven_home))?;
        let Some(record) = main_session::get_main_session(&conn, &scope)? else {
            continue;
        };
        if record.harness != "claude" {
            continue;
        }
        settle_open(coven_home, &conn, &record, now, &mut report)?;
        request_if_due(coven_home, &conn, &record, runtime, now, &mut report)?;
    }
    Ok(report)
}

/// The newest compaction of the pointer's current conversation.
pub fn latest(conn: &Connection, record: &MainSessionRecord) -> Result<Option<CompactionRecord>> {
    conn.query_row(
        "SELECT id, conversation_id, session_id, state, fill_tokens, window_tokens,
                requested_at, settled_at, pre_tokens, post_tokens, reason
         FROM main_session_compactions WHERE conversation_id = ?1
         ORDER BY requested_at DESC, rowid DESC LIMIT 1",
        [&record.conversation_id],
        |row| {
            Ok(CompactionRecord {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                session_id: row.get(2)?,
                state: row.get(3)?,
                fill_tokens: row.get(4)?,
                window_tokens: row.get(5)?,
                requested_at: row.get(6)?,
                settled_at: row.get(7)?,
                pre_tokens: row.get(8)?,
                post_tokens: row.get(9)?,
                reason: row.get(10)?,
            })
        },
    )
    .optional()
    .context("failed to read main-session compaction")
}

fn timestamp(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

/// Token counts are stored as SQLite integers.
fn sql_tokens(tokens: u64) -> i64 {
    i64::try_from(tokens).unwrap_or(i64::MAX)
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("invalid main-session compaction timestamp {value}"))?
        .with_timezone(&Utc))
}

fn request_if_due(
    coven_home: &Path,
    conn: &Connection,
    record: &MainSessionRecord,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
    report: &mut CompactionReport,
) -> Result<()> {
    let Some(context) = main_session_transcript::measure(conn, record)?.context else {
        return Ok(());
    };
    let previous = latest(conn, record)?;
    // The floor is the first reading after a compaction, whatever its level,
    // so it is recorded before any threshold or idleness check.
    let floor = match &previous {
        Some(previous) if previous.state == "compacted" => {
            match floor(conn, &previous.id, context.event_seq, context.tokens)? {
                Some(floor) => Some(floor),
                None => return Ok(()),
            }
        }
        _ => None,
    };
    let Some(session_id) = record.current_session_id.as_deref() else {
        return Ok(());
    };
    let Some(session) = store::get_session(conn, session_id)? else {
        return Ok(());
    };
    if session.status != "running" || !runtime.live_session_is_idle(&session.id)? {
        return Ok(());
    }
    let Some(window) = context.window.filter(|window| *window > 0) else {
        return Ok(());
    };
    if u128::from(context.tokens) * 100 < u128::from(window) * u128::from(THRESHOLD_PERCENT) {
        return Ok(());
    }
    if let Some(floor) = floor {
        let regrowth = u128::from(window) * u128::from(REGROWTH_PERCENT) / 100;
        if u128::from(context.tokens) < u128::from(floor) + regrowth {
            return Ok(());
        }
    }
    if let Some(previous) = &previous {
        match previous.state.as_str() {
            "requested" => return Ok(()),
            "failed" => {
                let settled = parse_timestamp(previous.settled_at.as_deref().unwrap_or_default())?;
                if now < settled + Duration::minutes(RETRY_AFTER_MINUTES) {
                    return Ok(());
                }
            }
            _ => {}
        }
    }

    let after_seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(rowid), 0) FROM events WHERE session_id = ?1",
        [&session.id],
        |row| row.get(0),
    )?;
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO main_session_compactions
         (id, scope_key, conversation_id, session_id, after_seq, fill_tokens,
          window_tokens, state, requested_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'requested', ?8)",
        params![
            id,
            record.scope_key,
            record.conversation_id,
            session.id,
            after_seq,
            sql_tokens(context.tokens),
            sql_tokens(window),
            timestamp(now),
        ],
    )
    .context("failed to record main-session compaction request")?;
    let body = json!({ "data": format!("/compact {COMPACT_INSTRUCTIONS}") }).to_string();
    let response = crate::api::record_input(coven_home, &session.id, Some(&body), runtime)?;
    if response.status != 202 {
        let reason = format!(
            "the host session refused the compaction input ({}): {}",
            response.status,
            bounded(&response.body, REASON_MAX_BYTES)
        );
        fail(conn, &id, &reason, now)?;
        report.failed.push((id, reason));
    }
    Ok(())
}

/// The first fill reading after compaction `id`, recorded once it exists.
fn floor(conn: &Connection, id: &str, reading_seq: i64, reading: u64) -> Result<Option<u64>> {
    let (floor, boundary_seq): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT floor_tokens, boundary_seq FROM main_session_compactions WHERE id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if let Some(floor) = floor {
        return Ok(Some(u64::try_from(floor).unwrap_or_default()));
    }
    if boundary_seq.is_none_or(|boundary| reading_seq <= boundary) {
        return Ok(None);
    }
    conn.execute(
        "UPDATE main_session_compactions SET floor_tokens = ?2
         WHERE id = ?1 AND floor_tokens IS NULL",
        params![id, sql_tokens(reading)],
    )?;
    Ok(Some(reading))
}

struct Open {
    id: String,
    session_id: String,
    after_seq: i64,
    fill_tokens: i64,
    window_tokens: i64,
    requested_at: String,
}

/// What the host emitted after a request's cursor.
#[derive(Default)]
struct Evidence {
    boundary: Option<Boundary>,
    summary: Option<String>,
    /// The first `result` after the cursor ends the command.
    result: Option<CommandResult>,
}

struct Boundary {
    seq: i64,
    trigger: Option<String>,
    pre_tokens: Option<u64>,
    post_tokens: Option<u64>,
}

struct CommandResult {
    is_error: bool,
    text: String,
}

fn settle_open(
    coven_home: &Path,
    conn: &Connection,
    record: &MainSessionRecord,
    now: DateTime<Utc>,
    report: &mut CompactionReport,
) -> Result<()> {
    let open = conn
        .query_row(
            "SELECT id, session_id, after_seq, fill_tokens, window_tokens, requested_at
             FROM main_session_compactions
             WHERE conversation_id = ?1 AND state = 'requested'",
            [&record.conversation_id],
            |row| {
                Ok(Open {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    after_seq: row.get(2)?,
                    fill_tokens: row.get(3)?,
                    window_tokens: row.get(4)?,
                    requested_at: row.get(5)?,
                })
            },
        )
        .optional()
        .context("failed to read open main-session compaction")?;
    let Some(open) = open else {
        return Ok(());
    };
    let evidence = evidence(conn, &open.session_id, open.after_seq)?;
    let reason = match (&evidence.result, evidence.boundary) {
        (Some(_), Some(boundary)) => {
            return compacted(
                coven_home,
                conn,
                record,
                &open,
                boundary,
                evidence.summary,
                now,
            );
        }
        (Some(result), None) => {
            let outcome = if result.is_error {
                "failed"
            } else {
                "finished"
            };
            format!(
                "Claude {outcome} /compact without compacting: {}",
                bounded(&result.text, REASON_MAX_BYTES)
            )
        }
        (None, _) => {
            let host_running = store::get_session(conn, &open.session_id)?
                .is_some_and(|session| session.status == "running");
            if !host_running {
                "the host session ended before the compaction finished".to_owned()
            } else if now
                >= parse_timestamp(&open.requested_at)? + Duration::minutes(DEADLINE_MINUTES)
            {
                format!("no compaction result within {DEADLINE_MINUTES} minutes")
            } else {
                return Ok(());
            }
        }
    };
    fail(conn, &open.id, &reason, now)?;
    report.failed.push((open.id, reason));
    Ok(())
}

/// Reads the host's output after `after_seq`, in stream order, up to the
/// first `result` or the scan budget.
fn evidence(conn: &Connection, session_id: &str, after_seq: i64) -> Result<Evidence> {
    let mut statement = conn.prepare(
        "SELECT rowid, octet_length(payload_json) FROM events
         WHERE session_id = ?1 AND kind = 'output' AND rowid > ?2 ORDER BY rowid",
    )?;
    let window = statement
        .query_map(params![session_id, after_seq], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut evidence = Evidence::default();
    let mut lines = OutputLines::default();
    let mut bytes = 0u64;
    for (seq, size) in window.into_iter().take(SCAN_MAX_EVENTS) {
        bytes = bytes.saturating_add(u64::try_from(size).unwrap_or_default());
        if bytes > SCAN_MAX_BYTES {
            break;
        }
        let payload = main_session_transcript::event_payload(conn, seq)?;
        for line in lines.feed(&payload) {
            let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            match frame.get("type").and_then(Value::as_str) {
                Some("system")
                    if frame.get("subtype").and_then(Value::as_str) == Some("compact_boundary") =>
                {
                    let metadata = frame.get("compact_metadata");
                    let field = |key: &str| metadata.and_then(|metadata| metadata.get(key));
                    evidence.boundary = Some(Boundary {
                        seq,
                        trigger: field("trigger").and_then(Value::as_str).map(str::to_owned),
                        pre_tokens: field("pre_tokens").and_then(Value::as_u64),
                        post_tokens: field("post_tokens").and_then(Value::as_u64),
                    });
                }
                Some("user") if frame.get("isSynthetic").and_then(Value::as_bool) == Some(true) => {
                    evidence.summary = frame
                        .get("message")
                        .and_then(|message| message.get("content"))
                        .and_then(message_text);
                }
                Some("result") => {
                    evidence.result = Some(CommandResult {
                        is_error: frame.get("is_error").and_then(Value::as_bool) == Some(true),
                        text: frame
                            .get("result")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    });
                    return Ok(evidence);
                }
                _ => {}
            }
        }
    }
    Ok(evidence)
}

/// A user message's text: a plain string, or the text blocks of a block list.
fn message_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn compacted(
    coven_home: &Path,
    conn: &Connection,
    record: &MainSessionRecord,
    open: &Open,
    boundary: Boundary,
    summary: Option<String>,
    now: DateTime<Utc>,
) -> Result<()> {
    let summary_truncated = summary
        .as_ref()
        .is_some_and(|summary| summary.len() > SUMMARY_MAX_BYTES);
    let summary = summary.map(|summary| bounded(&summary, SUMMARY_MAX_BYTES));
    let transaction = conn.unchecked_transaction()?;
    let updated = transaction.execute(
        "UPDATE main_session_compactions
         SET state = 'compacted', settled_at = ?2, boundary_seq = ?3,
             pre_tokens = ?4, post_tokens = ?5
         WHERE id = ?1 AND state = 'requested'",
        params![
            open.id,
            timestamp(now),
            boundary.seq,
            boundary.pre_tokens.map(sql_tokens),
            boundary.post_tokens.map(sql_tokens)
        ],
    )?;
    if updated == 1 {
        // The summary is conversation content, so it goes through the
        // instance's privacy policy like every other persisted event.
        store::insert_event_with_privacy(
            &transaction,
            coven_home,
            &store::EventRecord {
                seq: 0,
                id: Uuid::new_v4().to_string(),
                session_id: open.session_id.clone(),
                kind: EVENT_KIND_COMPACTED.to_owned(),
                payload_json: json!({
                    "scopeKey": record.scope_key,
                    "conversationId": record.conversation_id,
                    "compactionId": open.id,
                    "trigger": boundary.trigger,
                    "fillTokens": open.fill_tokens,
                    "windowTokens": open.window_tokens,
                    "preTokens": boundary.pre_tokens,
                    "postTokens": boundary.post_tokens,
                    "summary": summary,
                    "summaryTruncated": summary_truncated,
                })
                .to_string(),
                created_at: timestamp(now),
            },
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn fail(conn: &Connection, id: &str, reason: &str, now: DateTime<Utc>) -> Result<()> {
    conn.execute(
        "UPDATE main_session_compactions SET state = 'failed', settled_at = ?2, reason = ?3
         WHERE id = ?1 AND state = 'requested'",
        params![id, timestamp(now), reason],
    )
    .context("failed to record main-session compaction failure")?;
    Ok(())
}

/// At most `max` bytes of `text`, cut on a character boundary.
fn bounded(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::handle_request_with_runtime;
    use crate::automations::delivery::tests::HomeRuntime;

    const MODEL: &str = "claude-opus-5-5";
    const WINDOW: u64 = 1_000_000;

    struct Fixture {
        home: tempfile::TempDir,
        _root: tempfile::TempDir,
        runtime: HomeRuntime,
        session: String,
    }

    impl Fixture {
        fn new() -> Self {
            let home = tempfile::tempdir().unwrap();
            std::fs::write(
                home.path().join("familiars.toml"),
                "[[familiar]]\nid = \"nova\"\ndisplay_name = \"Nova\"\nrole = \"Lead\"\n\
                 description = \"Fronts Home.\"\n",
            )
            .unwrap();
            let root = tempfile::tempdir().unwrap();
            let project = root.path().canonicalize().unwrap();
            let mut fixture = Self {
                home,
                _root: root,
                runtime: HomeRuntime::default(),
                session: String::new(),
            };
            let (status, body) = fixture.call(
                "POST",
                "/api/v1/main-session/turn",
                Some(json!({ "prompt": "good morning", "projectRoot": project })),
            );
            assert_eq!(status, 201, "{body}");
            fixture.session = body["session"]["id"].as_str().unwrap().to_owned();
            fixture
        }

        fn call(&self, method: &str, route: &str, body: Option<Value>) -> (u16, Value) {
            let body = body.map(|body| body.to_string());
            let response = handle_request_with_runtime(
                method,
                route,
                self.home.path(),
                None,
                body.as_deref(),
                &self.runtime,
            )
            .unwrap();
            (
                response.status,
                serde_json::from_str(&response.body).unwrap(),
            )
        }

        fn conn(&self) -> Connection {
            store::open_store(&store_path(self.home.path())).unwrap()
        }

        fn output(&self, data: &str) -> i64 {
            let conn = self.conn();
            store::insert_event(
                &conn,
                &store::EventRecord {
                    seq: 0,
                    id: Uuid::new_v4().to_string(),
                    session_id: self.session.clone(),
                    kind: "output".to_owned(),
                    payload_json: json!({ "data": data }).to_string(),
                    created_at: timestamp(at(0)),
                },
            )
            .unwrap();
            store::latest_event_seq(&conn, &self.session).unwrap()
        }

        /// A finished turn whose model call read `fill` tokens.
        fn turn_reading(&self, fill: u64) {
            let usage = json!({"type": "assistant", "parent_tool_use_id": null, "message": {
                "model": MODEL, "usage": {"input_tokens": 2, "cache_read_input_tokens": fill - 2,
                "cache_creation_input_tokens": 0, "output_tokens": 5}}});
            let result = json!({"type": "result", "subtype": "success", "is_error": false,
                "result": "ok", "modelUsage": {MODEL: {"contextWindow": WINDOW}}});
            self.output(&format!("{usage}\n{result}\n"));
        }

        /// What Claude Code 2.1 emits for `/compact` in stream mode, trimmed.
        /// Returns the cursor of the event holding the boundary.
        fn compaction_frames(&self, summary: &str) -> i64 {
            let compacting = json!({"type": "system", "subtype": "status", "status": "compacting"});
            let boundary = json!({"type": "system", "subtype": "compact_boundary",
                "compact_metadata": {"trigger": "manual", "pre_tokens": 600_010,
                "post_tokens": 2_310, "duration_ms": 5_144}});
            let synthetic = json!({"type": "user", "isSynthetic": true,
                "message": {"role": "user", "content": summary}});
            let result = json!({"type": "result", "subtype": "success", "is_error": false,
                "local_command": "compact", "result": "", "num_turns": 0});
            let boundary_seq = self.output(&format!("{compacting}\n{boundary}\n"));
            let line = format!("{synthetic}\n");
            let mut middle = line.len() / 2;
            while !line.is_char_boundary(middle) {
                middle -= 1;
            }
            let (head, tail) = line.split_at(middle);
            self.output(head);
            self.output(tail);
            self.output(&format!("{result}\n"));
            boundary_seq
        }

        fn tick(&self, now: DateTime<Utc>) -> CompactionReport {
            tick(self.home.path(), &self.runtime, now).unwrap()
        }

        fn rows(&self) -> Vec<(String, Option<String>)> {
            let conn = self.conn();
            let mut statement = conn
                .prepare("SELECT state, reason FROM main_session_compactions ORDER BY rowid")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        }

        fn compacted_events(&self) -> Vec<Value> {
            store::list_events(&self.conn(), &self.session)
                .unwrap()
                .into_iter()
                .filter(|event| event.kind == EVENT_KIND_COMPACTED)
                .map(|event| serde_json::from_str(&event.payload_json).unwrap())
                .collect()
        }

        fn compact_inputs(&self) -> usize {
            self.runtime
                .inputs
                .borrow()
                .iter()
                .filter(|input| input.starts_with("/compact "))
                .count()
        }
    }

    fn at(minutes: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-18T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
            + Duration::minutes(minutes)
    }

    #[test]
    fn an_idle_home_is_sent_compact_once_its_fill_reaches_the_threshold() {
        let f = Fixture::new();
        f.turn_reading(599_999);
        f.tick(at(0));
        assert!(f.rows().is_empty(), "59.9999% is below the threshold");

        f.runtime.busy.set(true);
        f.turn_reading(600_000);
        f.tick(at(1));
        assert!(
            f.rows().is_empty(),
            "a turn still generating is never interrupted"
        );

        f.runtime.busy.set(false);
        f.tick(at(2));
        assert_eq!(f.rows(), vec![("requested".to_owned(), None)]);
        assert_eq!(
            f.runtime.inputs.borrow().last().unwrap(),
            &format!("/compact {COMPACT_INSTRUCTIONS}")
        );
        f.tick(at(3));
        assert_eq!(f.rows().len(), 1, "one open compaction per conversation");
        assert_eq!(f.compact_inputs(), 1);
    }

    #[test]
    fn a_compaction_settles_from_its_frames_into_the_event_stream() {
        let f = Fixture::new();
        f.turn_reading(650_000);
        f.tick(at(0));
        let boundary_seq = f.compaction_frames(
            "This session is being continued from a previous conversation.\n\nSummary:\n\
             Decisions: ship slice 2.\nExact identifiers: codeword PLUM-42",
        );

        assert_eq!(f.tick(at(1)), CompactionReport::default());
        assert_eq!(f.rows(), vec![("compacted".to_owned(), None)]);
        let events = f.compacted_events();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event["trigger"], "manual");
        assert_eq!(event["fillTokens"], 650_000);
        assert_eq!(event["windowTokens"], WINDOW);
        assert_eq!(event["preTokens"], 600_010);
        assert_eq!(event["postTokens"], 2_310);
        assert_eq!(event["summaryTruncated"], false);
        assert!(event["summary"]
            .as_str()
            .unwrap()
            .contains("codeword PLUM-42"));

        let (_, get) = f.call("GET", "/api/v1/main-session", None);
        assert_eq!(get["lastCompaction"]["state"], "compacted");
        assert_eq!(get["lastCompaction"]["postTokens"], 2_310);
        assert_eq!(get["transcript"]["context"], Value::Null);
        let boundary: i64 = f
            .conn()
            .query_row(
                "SELECT boundary_seq FROM main_session_compactions",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(boundary, boundary_seq);

        f.tick(at(2));
        assert_eq!(f.compacted_events().len(), 1, "settled exactly once");
    }

    #[test]
    fn a_result_without_a_boundary_fails_and_holds_off_retries() {
        let f = Fixture::new();
        f.turn_reading(700_000);
        f.tick(at(0));
        let refusal = json!({"type": "result", "subtype": "success", "is_error": false,
            "local_command": "compact", "result": "Not enough messages to compact."});
        f.output(&format!("{refusal}\n"));

        let report = f.tick(at(1));
        assert_eq!(report.failed.len(), 1);
        let reason = "Claude finished /compact without compacting: Not enough messages to compact.";
        assert_eq!(report.failed[0].1, reason);
        assert_eq!(
            f.rows(),
            vec![("failed".to_owned(), Some(reason.to_owned()))]
        );
        assert!(f.compacted_events().is_empty());

        f.tick(at(1 + RETRY_AFTER_MINUTES - 1));
        assert_eq!(f.compact_inputs(), 1, "no retry inside the hold-off");
        f.tick(at(1 + RETRY_AFTER_MINUTES));
        assert_eq!(f.compact_inputs(), 2);
    }

    #[test]
    fn a_stopped_or_silent_host_fails_the_request() {
        let f = Fixture::new();
        f.turn_reading(700_000);
        f.tick(at(0));
        f.tick(at(DEADLINE_MINUTES - 1));
        assert_eq!(f.rows()[0].0, "requested", "still inside the deadline");
        f.tick(at(DEADLINE_MINUTES));
        assert_eq!(
            f.rows()[0],
            (
                "failed".to_owned(),
                Some(format!(
                    "no compaction result within {DEADLINE_MINUTES} minutes"
                ))
            )
        );

        let f = Fixture::new();
        f.turn_reading(700_000);
        f.tick(at(0));
        let conn = f.conn();
        store::update_session_status(&conn, &f.session, "failed", Some(1), &timestamp(at(1)))
            .unwrap();
        f.tick(at(1));
        assert_eq!(
            f.rows()[0].1.as_deref(),
            Some("the host session ended before the compaction finished")
        );
    }

    #[test]
    fn the_first_reading_after_a_compaction_floors_the_next_one() {
        let f = Fixture::new();
        f.turn_reading(650_000);
        f.tick(at(0));
        f.compaction_frames("Summary: nothing yet.");
        f.tick(at(1));
        assert_eq!(f.compact_inputs(), 1);

        // The system prompt and tools survive compaction: a baseline this
        // large is above the threshold on its own and must not loop.
        f.turn_reading(700_000);
        f.tick(at(2));
        f.turn_reading(799_999);
        f.tick(at(3));
        assert_eq!(f.compact_inputs(), 1, "below the floor plus regrowth");
        f.turn_reading(800_000);
        f.tick(at(4));
        assert_eq!(f.compact_inputs(), 2);
        assert_eq!(f.rows().len(), 2);
    }

    #[test]
    fn a_compaction_that_frees_room_compacts_again_at_the_threshold() {
        let f = Fixture::new();
        f.turn_reading(650_000);
        f.tick(at(0));
        f.compaction_frames("Summary: nothing yet.");
        f.tick(at(1));
        // The usual case: the fill drops well below the threshold, and the
        // floor is that first reading even though nothing is due yet.
        f.turn_reading(300_000);
        f.tick(at(2));
        f.turn_reading(600_000);
        f.tick(at(3));
        assert_eq!(f.compact_inputs(), 2, "60% again, not 60% plus regrowth");
    }

    #[test]
    fn the_summary_in_the_event_is_bounded() {
        let f = Fixture::new();
        f.turn_reading(650_000);
        f.tick(at(0));
        // Two-byte characters after one ASCII byte: the cap lands mid-character.
        f.compaction_frames(&format!("a{}", "é".repeat(SUMMARY_MAX_BYTES)));
        f.tick(at(1));
        let event = &f.compacted_events()[0];
        assert_eq!(event["summaryTruncated"], true);
        assert_eq!(
            event["summary"].as_str().unwrap().len(),
            SUMMARY_MAX_BYTES - 1
        );
    }
}
