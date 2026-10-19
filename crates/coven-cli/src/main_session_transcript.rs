//! Transcript accounting for the main conversation (coven#1181, epic #1183).
//!
//! Compaction has to know how close Home is to its harness's context limit.
//! Nothing extra is recorded for that: the measure is derived from the events
//! the daemon already stores, scoped exactly like the main-session event
//! stream (every session row carrying the pointer's conversation id on its
//! harness).
//!
//! - `events` and `bytes` count the conversation's `input` and `output` events
//!   and their stored payload size. This is an approximation of transcript
//!   size: the prompt that launches a process goes to the harness rather than
//!   into an event, and retention can delete history.
//! - `context` is the occupancy the harness last reported. Claude's
//!   stream-json reports each model call's usage on its `assistant` frame, and
//!   the whole prompt is uncached input plus cache reads plus cache writes.
//!   Frames from a subagent (`parent_tool_use_id` set) describe another
//!   context and are skipped. The window comes from a `result` frame's
//!   `modelUsage` entry for that model. A `compact_boundary` frame newer than
//!   the last usage report makes that report stale, and its `post_tokens`
//!   counts only the summarised messages, not the system prompt and tools, so
//!   it is no fill reading either: `context` is `None` until the next model
//!   call reports. Codex's plain-text output reports nothing, so its
//!   `context` is `None` too.
//!
//! Reports are found by reading the conversation's newest output events
//! backwards within a fixed budget and reassembling stdout lines per session,
//! because one line can span several events. A report older than the budget
//! reads as unknown rather than stale.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::main_session::MainSessionRecord;

/// Newest output events read when looking for harness reports.
const SCAN_MAX_EVENTS: usize = 512;
/// Stored payload bytes read when looking for harness reports.
const SCAN_MAX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptMeasure {
    /// Retained `input` and `output` events of the conversation.
    pub events: u64,
    /// Stored payload bytes of those events.
    pub bytes: u64,
    /// The context occupancy the harness last reported, if any.
    pub context: Option<ReportedContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportedContext {
    /// Tokens occupying the context window.
    pub tokens: u64,
    /// The model's context window, when the harness reported one.
    pub window: Option<u64>,
    pub model: Option<String>,
    /// Cursor of the event that completed the report, as the event stream
    /// numbers it.
    pub event_seq: i64,
    pub reported_at: String,
}

pub fn measure(conn: &Connection, record: &MainSessionRecord) -> Result<TranscriptMeasure> {
    measure_within(conn, record, SCAN_MAX_EVENTS, SCAN_MAX_BYTES)
}

fn measure_within(
    conn: &Connection,
    record: &MainSessionRecord,
    max_events: usize,
    max_bytes: u64,
) -> Result<TranscriptMeasure> {
    let (events, bytes): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(octet_length(e.payload_json)), 0)
             FROM events e JOIN sessions s ON s.id = e.session_id
             WHERE s.conversation_id = ?1 AND s.harness = ?2
               AND e.kind IN ('input', 'output')",
            params![record.conversation_id, record.harness],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .context("failed to measure main-session transcript")?;
    Ok(TranscriptMeasure {
        events: u64::try_from(events).unwrap_or_default(),
        bytes: u64::try_from(bytes).unwrap_or_default(),
        context: reported_context(conn, record, max_events, max_bytes)?,
    })
}

/// One reassembled stdout line that carries a report, tagged with the event
/// holding its newline.
struct Frame {
    seq: i64,
    created_at: String,
    report: Report,
}

enum Report {
    Usage { tokens: u64, model: Option<String> },
    Compaction,
    Windows(Map<String, Value>),
}

fn reported_context(
    conn: &Connection,
    record: &MainSessionRecord,
    max_events: usize,
    max_bytes: u64,
) -> Result<Option<ReportedContext>> {
    let mut sessions = conn.prepare(
        "SELECT id FROM sessions WHERE conversation_id = ?1 AND harness = ?2
         ORDER BY created_at DESC, rowid DESC",
    )?;
    let sessions = sessions
        .query_map(params![record.conversation_id, record.harness], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to list main-session transcript sessions")?;

    let mut budget = Budget {
        events: max_events,
        bytes: max_bytes,
    };
    let mut usage: Option<(Frame, u64, Option<String>)> = None;
    let mut windows: Option<Map<String, Value>> = None;
    'sessions: for session in sessions {
        let (frames, exhausted) = session_frames(conn, &session, &mut budget)?;
        // Newest first: the first report of each kind wins.
        for frame in frames.into_iter().rev() {
            match frame.report {
                // The newest reading predates a compaction: stale, and the
                // compaction itself reports no fill.
                Report::Compaction if usage.is_none() => return Ok(None),
                Report::Usage { tokens, ref model } if usage.is_none() => {
                    let model = model.clone();
                    usage = Some((frame, tokens, model));
                }
                Report::Windows(ref map) if windows.is_none() => windows = Some(map.clone()),
                _ => {}
            }
            if usage.is_some() && windows.is_some() {
                break 'sessions;
            }
        }
        if exhausted {
            break;
        }
    }

    let Some((frame, tokens, model)) = usage else {
        return Ok(None);
    };
    let window = model
        .as_deref()
        .zip(windows.as_ref())
        .and_then(|(model, windows)| windows.get(model))
        .and_then(|usage| usage.get("contextWindow"))
        .and_then(Value::as_u64);
    Ok(Some(ReportedContext {
        tokens,
        window,
        model,
        event_seq: frame.seq,
        reported_at: frame.created_at,
    }))
}

struct Budget {
    events: usize,
    bytes: u64,
}

/// Reassembles one session's newest output within the budget and returns its
/// report frames in stream order, plus whether the budget ran out before the
/// session's oldest output event.
fn session_frames(
    conn: &Connection,
    session: &str,
    budget: &mut Budget,
) -> Result<(Vec<Frame>, bool)> {
    // Select identities and sizes only; payloads load one row at a time
    // below, so an oversized history is never materialised.
    let mut statement = conn.prepare(
        "SELECT rowid, octet_length(payload_json), created_at FROM events
         WHERE session_id = ?1 AND kind = 'output' ORDER BY rowid DESC",
    )?;
    let mut rows = statement.query([session])?;
    let mut window = Vec::new();
    let mut exhausted = false;
    while let Some(row) = rows.next()? {
        let size = u64::try_from(row.get::<_, i64>(1)?).unwrap_or_default();
        if budget.events == 0 || size > budget.bytes {
            exhausted = true;
            break;
        }
        budget.events -= 1;
        budget.bytes -= size;
        window.push((row.get::<_, i64>(0)?, row.get::<_, String>(2)?));
    }
    drop(rows);

    // A window that stops short of the session's first event can start
    // mid-line. That fragment never parses: a proper suffix of a one-object
    // line always carries an unmatched closing brace.
    let mut frames = Vec::new();
    let mut lines = OutputLines::default();
    for (seq, created_at) in window.into_iter().rev() {
        let payload = event_payload(conn, seq)?;
        for line in lines.feed(&payload) {
            if let Some(report) = report(&line) {
                frames.push(Frame {
                    seq,
                    created_at: created_at.clone(),
                    report,
                });
            }
        }
    }
    Ok((frames, exhausted))
}

pub(crate) fn event_payload(conn: &Connection, seq: i64) -> Result<String> {
    conn.query_row(
        "SELECT payload_json FROM events WHERE rowid = ?1",
        [seq],
        |row| row.get(0),
    )
    .context("failed to read main-session transcript event")
}

/// Reassembles stdout lines from one session's consecutive `output` event
/// payloads. The coalescing event writer can split a line across events or
/// put several lines in one.
#[derive(Default)]
pub(crate) struct OutputLines {
    line: String,
}

impl OutputLines {
    /// Feeds one stored payload and returns the lines it completes. A trailing
    /// unterminated line stays open: it is still being written.
    pub(crate) fn feed(&mut self, payload: &str) -> Vec<String> {
        let Some(data) = serde_json::from_str::<Value>(payload)
            .ok()
            .and_then(|payload| payload.get("data")?.as_str().map(str::to_owned))
        else {
            // Not stored stdout text: whatever line was open cannot be trusted.
            self.line.clear();
            return Vec::new();
        };
        let mut complete = Vec::new();
        let mut rest = data.as_str();
        while let Some(end) = rest.find('\n') {
            self.line.push_str(&rest[..end]);
            rest = &rest[end + 1..];
            complete.push(std::mem::take(&mut self.line));
        }
        self.line.push_str(rest);
        complete
    }
}

fn report(line: &str) -> Option<Report> {
    let frame: Value = serde_json::from_str(line).ok()?;
    match frame.get("type")?.as_str()? {
        "assistant" if frame.get("parent_tool_use_id").is_none_or(Value::is_null) => {
            let message = frame.get("message")?;
            let usage = message.get("usage")?;
            let count = |key: &str| usage.get(key).and_then(Value::as_u64);
            let tokens = count("input_tokens")?
                .saturating_add(count("cache_read_input_tokens").unwrap_or_default())
                .saturating_add(count("cache_creation_input_tokens").unwrap_or_default());
            let model = message
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned);
            Some(Report::Usage { tokens, model })
        }
        "system" if frame.get("subtype")?.as_str()? == "compact_boundary" => {
            Some(Report::Compaction)
        }
        "result" => Some(Report::Windows(
            frame.get("modelUsage")?.as_object()?.clone(),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::store_path,
        main_session::{self, INSTANCE_MAIN_SCOPE_KEY},
        store,
    };
    use serde_json::json;

    const NOW: &str = "2026-10-18T12:00:00Z";
    const MODEL: &str = "claude-haiku-5-5";

    struct Home {
        _dir: tempfile::TempDir,
        conn: Connection,
        record: MainSessionRecord,
    }

    impl Home {
        fn new(harness: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let mut conn = store::open_store(&store_path(dir.path())).unwrap();
            let record = main_session::resolve_main_session(
                &mut conn,
                INSTANCE_MAIN_SCOPE_KEY,
                None,
                harness,
                "/tmp/project",
                NOW,
            )
            .unwrap()
            .record;
            Self {
                _dir: dir,
                conn,
                record,
            }
        }

        fn session(&self, id: &str, conversation: &str, harness: &str, created_at: &str) {
            self.conn
                .execute(
                    "INSERT INTO sessions
                     (id, project_root, harness, title, status, created_at, updated_at, conversation_id)
                     VALUES (?1, '/tmp/project', ?2, 'Home', 'running', ?3, ?3, ?4)",
                    params![id, harness, created_at, conversation],
                )
                .unwrap();
        }

        fn event(&self, session: &str, kind: &str, payload: Value) -> i64 {
            store::insert_event(
                &self.conn,
                &store::EventRecord {
                    seq: 0,
                    id: uuid::Uuid::new_v4().to_string(),
                    session_id: session.to_owned(),
                    kind: kind.to_owned(),
                    payload_json: payload.to_string(),
                    created_at: NOW.to_owned(),
                },
            )
            .unwrap();
            store::latest_event_seq(&self.conn, session).unwrap()
        }

        fn output(&self, session: &str, data: &str) -> i64 {
            self.event(session, "output", json!({ "data": data }))
        }

        fn measure(&self) -> TranscriptMeasure {
            measure(&self.conn, &self.record).unwrap()
        }
    }

    /// Frame shapes captured from Claude Code 2.1 `--output-format
    /// stream-json --verbose`, trimmed to the fields that matter here.
    fn assistant(input: u64, cache_read: u64, cache_write: u64, parent: Option<&str>) -> String {
        json!({
            "type": "assistant",
            "message": {
                "model": MODEL,
                "role": "assistant",
                "content": [{"type": "text", "text": "OK"}],
                "usage": {
                    "input_tokens": input,
                    "cache_read_input_tokens": cache_read,
                    "cache_creation_input_tokens": cache_write,
                    "output_tokens": 4
                }
            },
            "parent_tool_use_id": parent,
            "session_id": "s"
        })
        .to_string()
    }

    fn result(window: u64) -> String {
        json!({
            "type": "result",
            "subtype": "success",
            // A turn total across model calls; never a fill reading.
            "usage": {"input_tokens": 9, "cache_read_input_tokens": 900000},
            "modelUsage": {MODEL: {"inputTokens": 9, "contextWindow": window}}
        })
        .to_string()
    }

    fn compact_boundary(pre: u64, post: u64) -> String {
        json!({
            "type": "system",
            "subtype": "compact_boundary",
            "compact_metadata": {"trigger": "auto", "pre_tokens": pre, "post_tokens": post}
        })
        .to_string()
    }

    #[test]
    fn context_is_the_latest_top_level_model_call_and_its_window() {
        let h = Home::new("claude");
        let conversation = h.record.conversation_id.clone();
        h.session("home", &conversation, "claude", NOW);
        h.output(
            "home",
            &format!("{}\n{}\n", assistant(1, 10, 100, None), result(200_000)),
        );
        h.event("home", "input", json!({ "data": "next question" }));
        // One stdout line split across three events, then a subagent call
        // whose context is not Home's.
        let line = format!("{}\n", assistant(2, 2_000, 300, None));
        let (a, rest) = line.split_at(40);
        let (b, c) = rest.split_at(60);
        h.output("home", a);
        h.output("home", b);
        let completed = h.output("home", c);
        h.output(
            "home",
            &format!("{}\n", assistant(7, 7, 7, Some("toolu_1"))),
        );
        h.output("home", &format!("{}\n", result(1_000_000)));
        // An unterminated line is still being written and reports nothing.
        h.output("home", &compact_boundary(5_000, 1)[..30]);

        let measure = h.measure();
        assert_eq!(measure.events, 8);
        let context = measure.context.expect("usage was reported");
        assert_eq!(context.tokens, 2_302);
        assert_eq!(context.window, Some(1_000_000));
        assert_eq!(context.model.as_deref(), Some(MODEL));
        assert_eq!(context.event_seq, completed);
        assert_eq!(context.reported_at, NOW);
    }

    #[test]
    fn a_compaction_leaves_context_unknown_until_the_next_model_call() {
        let h = Home::new("claude");
        let conversation = h.record.conversation_id.clone();
        h.session("home", &conversation, "claude", NOW);
        h.output(
            "home",
            &format!(
                "{}\n{}\n",
                assistant(2, 0, 280_912, None),
                result(1_000_000)
            ),
        );
        // Captured numbers: post_tokens counts only the summary, while the
        // next model call, system prompt and tools included, read 305,499.
        h.output("home", &format!("{}\n", compact_boundary(280_918, 2_310)));
        assert_eq!(h.measure().context, None);

        let next = h.output(
            "home",
            &format!("{}\n", assistant(2, 270_546, 34_951, None)),
        );
        let context = h.measure().context.unwrap();
        assert_eq!(context.tokens, 305_499);
        assert_eq!(context.event_seq, next);
        assert_eq!(context.window, Some(1_000_000));
    }

    #[test]
    fn only_the_current_conversation_on_its_harness_is_measured() {
        let h = Home::new("claude");
        let conversation = h.record.conversation_id.clone();
        h.session(
            "old",
            "previous-conversation",
            "claude",
            "2026-10-18T11:00:00Z",
        );
        h.output("old", &format!("{}\n", assistant(1, 900_000, 0, None)));
        h.session("codex", &conversation, "codex", "2026-10-18T11:30:00Z");
        h.output("codex", &format!("{}\n", assistant(1, 800_000, 0, None)));
        h.session("first", &conversation, "claude", "2026-10-18T11:45:00Z");
        h.output("first", &format!("{}\n", assistant(1, 40, 0, None)));
        // A later host of the same conversation without a report of its own:
        // the earlier host's report still describes the conversation.
        h.session("second", &conversation, "claude", NOW);
        h.event("second", "input", json!({ "data": "hello" }));
        h.event(
            "second",
            "exit",
            json!({ "status": "completed", "exitCode": 0 }),
        );

        let measure = h.measure();
        assert_eq!(
            measure.events, 2,
            "input and output only, this conversation only"
        );
        let context = measure.context.unwrap();
        assert_eq!(context.tokens, 41);
        assert_eq!(context.window, None, "no result frame reported a window");
    }

    #[test]
    fn plain_text_output_reports_size_but_no_context() {
        let h = Home::new("codex");
        let conversation = h.record.conversation_id.clone();
        h.session("home", &conversation, "codex", NOW);
        let payload = json!({ "data": "session id: 5f0d\nhello there\n" });
        h.event("home", "output", payload.clone());

        let measure = h.measure();
        assert_eq!(measure.events, 1);
        assert_eq!(measure.bytes, payload.to_string().len() as u64);
        assert_eq!(measure.context, None);
    }

    #[test]
    fn reports_older_than_the_scan_budget_read_as_unknown() {
        let h = Home::new("claude");
        let conversation = h.record.conversation_id.clone();
        h.session("home", &conversation, "claude", NOW);
        h.output(
            "home",
            &format!("{}\n{}\n", assistant(1, 10, 0, None), result(200_000)),
        );
        let filler = format!(
            "{}\n",
            json!({"type": "user", "message": {"content": "x".repeat(64)}})
        );
        for _ in 0..3 {
            h.output("home", &filler);
        }

        let unknown = measure_within(&h.conn, &h.record, 3, SCAN_MAX_BYTES).unwrap();
        assert_eq!(unknown.context, None);
        let found = measure_within(&h.conn, &h.record, 4, SCAN_MAX_BYTES).unwrap();
        assert_eq!(found.context.unwrap().tokens, 11);
        let bytes = u64::try_from(filler.len()).unwrap() * 3;
        assert_eq!(
            measure_within(&h.conn, &h.record, 512, bytes)
                .unwrap()
                .context,
            None,
            "the byte budget bounds the scan the same way"
        );
    }

    #[test]
    fn a_window_that_starts_mid_line_ignores_the_fragment() {
        let h = Home::new("claude");
        let conversation = h.record.conversation_id.clone();
        h.session("home", &conversation, "claude", NOW);
        let line = format!("{}\n", assistant(5, 5, 5, None));
        let (head, tail) = line.split_at(line.len() / 2);
        h.output("home", head);
        h.output("home", tail);

        let cut = measure_within(&h.conn, &h.record, 1, SCAN_MAX_BYTES).unwrap();
        assert_eq!(cut.context, None);
        let whole = measure_within(&h.conn, &h.record, 2, SCAN_MAX_BYTES).unwrap();
        assert_eq!(whole.context.unwrap().tokens, 15);
    }
}
