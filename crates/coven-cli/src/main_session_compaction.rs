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
//! 3. **Flush.** The instructions ask Claude to end the summary with a marked
//!    durable-facts block. Settlement cuts it out of the summary and appends
//!    it to the familiar's daily note, `<workspace>/memory/YYYY-MM-DD.md`.
//!    The daemon is the only writer: it appends, never truncates, refuses a
//!    symlinked memory directory or note, and marks each entry with the
//!    compaction id, so a settlement repeated after a crash appends nothing.
//!    A missing block or a refused write fails the flush, not the compaction.
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

use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Local, SecondsFormat, Utc};
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
const FACTS_OPEN: &str = "<durable-facts>";
const FACTS_CLOSE: &str = "</durable-facts>";
const MAX_FACTS: usize = 50;
const FACT_MAX_BYTES: usize = 1024;
/// Marks each daily-note entry with its compaction id.
const NOTE_MARKER: &str = "coven:main-session-compaction";
/// A note larger than this is not read back for the marker; the append goes
/// ahead, since the marker check only guards a repeated settlement.
const NOTE_SCAN_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Follows `/compact`. Claude adds these to its own summary instructions.
pub const COMPACT_INSTRUCTIONS: &str = "Coven is compacting Home, the long-running main \
     conversation. Write the summary under exactly these headings, in this order: Decisions, \
     Open TODOs, Constraints, Pending user asks, Exact identifiers. Under Exact identifiers, copy \
     verbatim every id, path, URL, name, number and codeword the conversation still depends on. \
     Finally, list the durable facts worth keeping beyond this conversation (decisions, \
     preferences, commitments and exact identifiers), one per line starting with \"- \", between \
     a line reading <durable-facts> and a line reading </durable-facts>. Write \"- none\" between \
     them if there are none.";

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
    /// `appended`, `empty` or `failed` once compacted.
    pub flush_state: Option<String>,
    /// The daily note, relative to the familiar's workspace.
    pub flush_path: Option<String>,
    pub flush_reason: Option<String>,
}

/// Stores that created `main_session_compactions` before the daily-note flush
/// gain its columns here. Idempotent; called after the CREATE TABLE.
pub fn ensure_compaction_flush_columns(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare("PRAGMA table_info(main_session_compactions)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to inspect main_session_compactions schema")?;
    for (column, definition) in [
        (
            "flush_state",
            "flush_state TEXT CHECK (flush_state IN ('appended', 'empty', 'failed'))",
        ),
        ("flush_path", "flush_path TEXT"),
        ("flush_reason", "flush_reason TEXT"),
    ] {
        if !columns.iter().any(|existing| existing == column) {
            conn.execute(
                &format!("ALTER TABLE main_session_compactions ADD COLUMN {definition}"),
                [],
            )
            .with_context(|| format!("failed to add main_session_compactions.{column}"))?;
        }
    }
    Ok(())
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
                requested_at, settled_at, pre_tokens, post_tokens, reason,
                flush_state, flush_path, flush_reason
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
                flush_state: row.get(11)?,
                flush_path: row.get(12)?,
                flush_reason: row.get(13)?,
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
    let (facts, summary) = match summary {
        Some(summary) => {
            let (facts, rest) = durable_facts(&summary);
            (facts, Some(rest))
        }
        None => (None, None),
    };
    let flush = match facts {
        None => Flush::Failed(format!(
            "the summary had no {FACTS_OPEN} section; no daily note was written"
        )),
        Some(facts) if facts.is_empty() => Flush::Empty,
        Some(facts) => match record.familiar_id.as_deref() {
            None => Flush::Failed("the main session has no familiar".to_owned()),
            Some(familiar) => {
                match append_daily_note(coven_home, familiar, &open.id, &facts, now) {
                    Ok(path) => Flush::Appended {
                        path,
                        facts: facts.len(),
                    },
                    Err(error) => Flush::Failed(bounded(&format!("{error:#}"), REASON_MAX_BYTES)),
                }
            }
        },
    };
    let summary_truncated = summary
        .as_ref()
        .is_some_and(|summary| summary.len() > SUMMARY_MAX_BYTES);
    let summary = summary.map(|summary| bounded(&summary, SUMMARY_MAX_BYTES));
    let transaction = conn.unchecked_transaction()?;
    let updated = transaction.execute(
        "UPDATE main_session_compactions
         SET state = 'compacted', settled_at = ?2, boundary_seq = ?3,
             pre_tokens = ?4, post_tokens = ?5,
             flush_state = ?6, flush_path = ?7, flush_reason = ?8
         WHERE id = ?1 AND state = 'requested'",
        params![
            open.id,
            timestamp(now),
            boundary.seq,
            boundary.pre_tokens.map(sql_tokens),
            boundary.post_tokens.map(sql_tokens),
            flush.state(),
            flush.path(),
            flush.reason(),
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
                    "flush": flush.value(),
                })
                .to_string(),
                created_at: timestamp(now),
            },
        )?;
    }
    transaction.commit()?;
    Ok(())
}

/// What became of a compaction's durable facts.
enum Flush {
    Appended {
        path: String,
        facts: usize,
    },
    /// Claude listed none.
    Empty,
    Failed(String),
}

impl Flush {
    fn state(&self) -> &'static str {
        match self {
            Flush::Appended { .. } => "appended",
            Flush::Empty => "empty",
            Flush::Failed(_) => "failed",
        }
    }

    fn path(&self) -> Option<&str> {
        match self {
            Flush::Appended { path, .. } => Some(path),
            _ => None,
        }
    }

    fn reason(&self) -> Option<&str> {
        match self {
            Flush::Failed(reason) => Some(reason),
            _ => None,
        }
    }

    fn value(&self) -> Value {
        match self {
            Flush::Appended { path, facts } => {
                json!({ "state": self.state(), "path": path, "facts": facts })
            }
            Flush::Empty => json!({ "state": self.state() }),
            Flush::Failed(reason) => json!({ "state": self.state(), "reason": reason }),
        }
    }
}

/// Splits the last durable-facts block out of a summary. Returns its facts
/// (`None` when there is no complete block; empty for `- none`) and the
/// summary without the block.
fn durable_facts(summary: &str) -> (Option<Vec<String>>, String) {
    let Some(open) = summary.rfind(FACTS_OPEN) else {
        return (None, summary.to_owned());
    };
    let body_start = open + FACTS_OPEN.len();
    let Some(close) = summary[body_start..].find(FACTS_CLOSE) else {
        return (None, summary.to_owned());
    };
    let body = &summary[body_start..body_start + close];
    let facts = body
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let fact = line
                .strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))
                .unwrap_or(line);
            let fact = clean_fact(fact);
            (!fact.is_empty() && !fact.eq_ignore_ascii_case("none")).then_some(fact)
        })
        .take(MAX_FACTS)
        .collect();
    let rest = format!(
        "{}{}",
        summary[..open].trim_end(),
        &summary[body_start + close + FACTS_CLOSE.len()..]
    );
    (Some(facts), rest.trim_end().to_owned())
}

/// One line of plain text: terminal escapes, control characters and line
/// separators removed, at most [`FACT_MAX_BYTES`].
fn clean_fact(fact: &str) -> String {
    let text: String = crate::automations::delivery::strip_escape_sequences(fact)
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '\u{2028}' | '\u{2029}'))
        .collect();
    bounded(text.trim(), FACT_MAX_BYTES)
}

fn note_relative_path(now: DateTime<Utc>) -> String {
    format!("memory/{}.md", now.with_timezone(&Local).format("%Y-%m-%d"))
}

/// Appends `facts` to the familiar's daily note and returns the note's path
/// relative to the workspace. The workspace must already exist; its
/// `memory` directory is created when missing and refused when it is a
/// symlink, and the note itself is opened without following a symlink.
fn append_daily_note(
    coven_home: &Path,
    familiar: &str,
    compaction_id: &str,
    facts: &[String],
    now: DateTime<Utc>,
) -> Result<String> {
    let workspace = crate::cockpit_sources::familiar_workspace(coven_home, familiar);
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("familiar {familiar} has no workspace directory"))?;
    if !workspace.is_dir() {
        bail!("familiar {familiar} has no workspace directory");
    }
    let memory = workspace.join("memory");
    match fs::symlink_metadata(&memory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            bail!("refusing a daily-note directory that is a symlink or not a directory")
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&memory).context("failed to create the daily-note directory")?;
        }
        Err(error) => return Err(error).context("failed to inspect the daily-note directory"),
    }
    let relative = note_relative_path(now);
    let path = workspace.join(&relative);
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        bail!("refusing a daily note that is a symlink");
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut note = options
        .open(&path)
        .context("failed to open the daily note; symlinks are refused")?;
    let marker = format!("<!-- {NOTE_MARKER} {compaction_id} -->");
    let mut existing = String::new();
    if note.metadata()?.len() <= NOTE_SCAN_MAX_BYTES {
        // Not UTF-8 means not ours to dedupe; the append still goes ahead.
        let _ = note.read_to_string(&mut existing);
    }
    if existing.contains(&marker) {
        return Ok(relative);
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let mut entry = format!(
        "{separator}{marker}\n### Home compaction, {}\n\n",
        now.with_timezone(&Local).format("%H:%M")
    );
    for fact in facts {
        entry.push_str("- ");
        entry.push_str(fact);
        entry.push('\n');
    }
    note.write_all(entry.as_bytes())
        .context("failed to append to the daily note")?;
    note.sync_data().context("failed to sync the daily note")?;
    Ok(relative)
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
            // Nova's default workspace, where daily notes go.
            fs::create_dir_all(home.path().join("familiars").join("nova")).unwrap();
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

        fn workspace(&self) -> std::path::PathBuf {
            self.home.path().join("familiars").join("nova")
        }

        /// The daily note a settlement at `at(1)` writes.
        fn note(&self) -> std::path::PathBuf {
            self.workspace().join(note_relative_path(at(1)))
        }

        /// Requests a compaction and settles it with `summary` at `at(1)`.
        fn compact_with(&self, summary: &str) -> Value {
            self.turn_reading(650_000);
            self.tick(at(0));
            self.compaction_frames(summary);
            self.tick(at(1));
            self.compacted_events()
                .pop()
                .expect("the compaction settled")
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

    #[test]
    fn durable_facts_land_once_in_the_familiars_daily_note() {
        let f = Fixture::new();
        let event = f.compact_with(
            "Summary:\nDecisions: ship slice 2.\n<durable-facts>\n- Val prefers terse updates.\n\
             - \u{1b}[31mcodeword PLUM-42\u{1b}[0m\n- none\n\n</durable-facts>\n",
        );
        let relative = note_relative_path(at(1));
        assert_eq!(
            event["flush"],
            json!({ "state": "appended", "path": relative, "facts": 2 })
        );
        let summary = event["summary"].as_str().unwrap();
        assert!(summary.contains("Decisions: ship slice 2."));
        assert!(!summary.contains(FACTS_OPEN), "the facts left the summary");

        let note = fs::read_to_string(f.note()).unwrap();
        let id = f.compacted_events()[0]["compactionId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(note.starts_with(&format!(
            "\n<!-- {NOTE_MARKER} {id} -->\n### Home compaction, "
        )));
        assert!(note.ends_with("\n\n- Val prefers terse updates.\n- codeword PLUM-42\n"));

        // A settlement repeated after a crash finds its marker and appends nothing.
        let facts = vec!["Val prefers terse updates.".to_owned()];
        append_daily_note(f.home.path(), "nova", &id, &facts, at(1)).unwrap();
        assert_eq!(fs::read_to_string(f.note()).unwrap(), note);

        let (_, get) = f.call("GET", "/api/v1/main-session", None);
        assert_eq!(get["lastCompaction"]["flushState"], "appended");
        assert_eq!(get["lastCompaction"]["flushPath"], relative);
    }

    #[test]
    fn an_existing_note_keeps_its_content() {
        let f = Fixture::new();
        fs::create_dir(f.workspace().join("memory")).unwrap();
        fs::write(f.note(), "# Today\n- morning standup").unwrap();
        f.compact_with("Summary.\n<durable-facts>\n- one fact\n</durable-facts>");
        let note = fs::read_to_string(f.note()).unwrap();
        assert!(
            note.starts_with("# Today\n- morning standup\n\n<!-- "),
            "{note}"
        );
        assert!(note.ends_with("- one fact\n"));
    }

    #[test]
    fn no_facts_or_no_block_writes_no_note() {
        let f = Fixture::new();
        let event = f.compact_with("Summary.\n<durable-facts>\n- none\n</durable-facts>");
        assert_eq!(event["flush"], json!({ "state": "empty" }));
        assert!(!f.note().exists());

        let f = Fixture::new();
        let event = f.compact_with("Summary without the block.");
        assert_eq!(event["flush"]["state"], "failed");
        assert_eq!(
            event["flush"]["reason"],
            "the summary had no <durable-facts> section; no daily note was written"
        );
        assert_eq!(f.rows(), vec![("compacted".to_owned(), None)]);
        assert!(!f.note().exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_note_or_memory_directory_is_refused() {
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("elsewhere.md");
        fs::write(&target, "untouched").unwrap();
        let block = "Summary.\n<durable-facts>\n- a fact\n</durable-facts>";

        let f = Fixture::new();
        fs::create_dir(f.workspace().join("memory")).unwrap();
        std::os::unix::fs::symlink(&target, f.note()).unwrap();
        let event = f.compact_with(block);
        assert_eq!(event["flush"]["state"], "failed");
        assert!(event["flush"]["reason"]
            .as_str()
            .unwrap()
            .contains("symlink"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");

        let f = Fixture::new();
        std::os::unix::fs::symlink(outside.path(), f.workspace().join("memory")).unwrap();
        let event = f.compact_with(block);
        assert_eq!(event["flush"]["state"], "failed");
        assert!(event["flush"]["reason"]
            .as_str()
            .unwrap()
            .contains("symlink"));
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
    }
}
