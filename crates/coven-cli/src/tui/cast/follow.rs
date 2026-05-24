//! Cast event follower.
//!
//! Phase 2 streams a launched daemon session into the Cast TUI as a
//! conversation transcript: each `output` event becomes a printable chunk,
//! each `exit` event marks completion, and a monotonic `afterSeq` cursor
//! ensures Cast never prints the same line twice across polls.
//!
//! The follower is intentionally decoupled from the wire transport. It
//! consumes the existing `tui::chat::client::ChatClient` trait, which lets
//! tests pass a stub implementation that returns canned events without
//! touching the daemon socket.

use anyhow::Result;
use serde_json::Value;

use crate::store;
use crate::tui::chat::client::{ChatClient, ChatEventQuery};

/// A monotonic cursor over a session's event stream. The cursor only ever
/// advances forward; passing the cursor back to the daemon as `afterSeq`
/// guarantees no duplicate delivery during normal polling.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CastFollowCursor {
    after_seq: Option<i64>,
}

impl CastFollowCursor {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn after_seq(&self) -> Option<i64> {
        self.after_seq
    }

    /// Advance the cursor to the maximum seq seen so far. Stale or
    /// out-of-order seqs are ignored so a misbehaving client cannot rewind.
    pub(crate) fn advance(&mut self, seq: i64) {
        if self.after_seq.map(|current| seq > current).unwrap_or(true) {
            self.after_seq = Some(seq);
        }
    }
}

/// The Cast-facing shape of a single daemon event. Cast only renders
/// the kinds it understands; unknown event kinds are surfaced as
/// `Other` so the follower can still advance the cursor without losing
/// future events.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CastFollowEvent {
    /// A chunk of harness output. Already extracted from the payload's
    /// `data` field; the renderer can print it verbatim.
    Output(String),
    /// The session finished. `status` is the lifecycle string the daemon
    /// records (`completed`, `failed`, `exited`); `exit_code` may be
    /// `None` if the harness did not produce one.
    Exit {
        status: String,
        exit_code: Option<i32>,
    },
    /// An event whose kind Cast does not specifically handle yet. The
    /// follower still advances the cursor past it so resumes work.
    Other { kind: String },
}

/// Translate a stored event record into the Cast follow shape. Pure
/// function — no IO, easy to unit-test against synthetic events.
pub(crate) fn decode_event(record: &store::EventRecord) -> CastFollowEvent {
    match record.kind.as_str() {
        "output" => {
            let data = parse_payload(&record.payload_json)
                .and_then(|value| {
                    value
                        .get("data")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .unwrap_or_default();
            CastFollowEvent::Output(data)
        }
        "exit" => {
            let payload = parse_payload(&record.payload_json);
            let status = payload
                .as_ref()
                .and_then(|value| value.get("status").and_then(Value::as_str))
                .unwrap_or("unknown")
                .to_string();
            let exit_code = payload
                .as_ref()
                .and_then(|value| {
                    value
                        .get("exitCode")
                        .and_then(Value::as_i64)
                        .map(|v| v as i32)
                })
                .or_else(|| {
                    payload.as_ref().and_then(|value| {
                        value
                            .get("exit_code")
                            .and_then(Value::as_i64)
                            .map(|v| v as i32)
                    })
                });
            CastFollowEvent::Exit { status, exit_code }
        }
        other => CastFollowEvent::Other {
            kind: other.to_string(),
        },
    }
}

/// Decoded follower batch returned by a single poll. Carries the cursor
/// the caller should use for the next request (or save in case it
/// reconnects after a crash).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CastFollowBatch {
    pub(crate) events: Vec<CastFollowEvent>,
    pub(crate) cursor: CastFollowCursor,
    pub(crate) exit: Option<CastSessionExit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CastSessionExit {
    pub(crate) status: String,
    pub(crate) exit_code: Option<i32>,
}

/// Observer for the running follower loop. The IO-heavy caller (stdout
/// writer, transcript renderer) implements this trait; the loop stays
/// free of side effects so tests can drive it with a recording observer
/// and a canned client.
pub(crate) trait FollowerObserver {
    fn on_output(&mut self, chunk: &str);
    fn on_exit(&mut self, status: &str, exit_code: Option<i32>);
    /// Cast doesn't render unknown event kinds today; observers can record
    /// them for debug overlays in a future phase.
    fn on_other(&mut self, _kind: &str) {}
}

/// Pacer for the follower poll loop. Production passes a `Duration` sleep;
/// tests pass a counter that returns `Err` after N idle polls so a stuck
/// session can't hang the suite.
pub(crate) trait FollowerPacer {
    /// Called after every poll that did not produce an exit. Implementations
    /// may sleep, count, or panic on iteration overflows. Returning an
    /// `Err` cleanly aborts the follower with that error.
    fn between_polls(&mut self) -> Result<()>;
}

/// Run the follower poll loop until the session emits an `exit` event or
/// the pacer returns an error. Each batch of events is decoded and pushed
/// to the observer; the cursor monotonically advances on every event so
/// duplicates are impossible.
pub(crate) fn follow_until_exit(
    client: &mut dyn ChatClient,
    session_id: &str,
    observer: &mut dyn FollowerObserver,
    pacer: &mut dyn FollowerPacer,
) -> Result<CastSessionExit> {
    let mut cursor = CastFollowCursor::new();
    loop {
        let batch = poll_once(client, session_id, cursor)?;
        cursor = batch.cursor;
        for event in &batch.events {
            match event {
                CastFollowEvent::Output(chunk) => observer.on_output(chunk),
                CastFollowEvent::Exit { status, exit_code } => {
                    observer.on_exit(status, *exit_code);
                }
                CastFollowEvent::Other { kind } => observer.on_other(kind),
            }
        }
        if let Some(exit) = batch.exit {
            return Ok(exit);
        }
        pacer.between_polls()?;
    }
}

/// Poll the daemon once for events past `cursor`, decode them, and return
/// a typed batch. The returned cursor reflects the maximum seq seen — pass
/// it back on the next call to avoid re-fetching events. If an `exit`
/// event lands in this batch, `batch.exit` is populated; the caller should
/// stop polling.
pub(crate) fn poll_once(
    client: &mut dyn ChatClient,
    session_id: &str,
    cursor: CastFollowCursor,
) -> Result<CastFollowBatch> {
    let records = client.list_events(ChatEventQuery {
        session_id,
        after_seq: cursor.after_seq(),
        limit: None,
    })?;

    let mut new_cursor = cursor;
    let mut events = Vec::with_capacity(records.len());
    let mut exit = None;
    for record in records {
        new_cursor.advance(record.seq);
        let decoded = decode_event(&record);
        if let CastFollowEvent::Exit { status, exit_code } = &decoded {
            exit = Some(CastSessionExit {
                status: status.clone(),
                exit_code: *exit_code,
            });
        }
        events.push(decoded);
    }

    Ok(CastFollowBatch {
        events,
        cursor: new_cursor,
        exit,
    })
}

fn parse_payload(payload_json: &str) -> Option<Value> {
    serde_json::from_str::<Value>(payload_json).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::chat::client::{ChatDaemonStatus, LaunchRequest};
    use std::cell::RefCell;

    /// Stub client that returns canned event batches in order. The first
    /// call returns `batches[0]`, the second returns `batches[1]`, etc.
    /// Test failures show the queries the follower made so we can verify
    /// cursor handoff.
    struct StubClient {
        batches: RefCell<Vec<Vec<store::EventRecord>>>,
        queries: RefCell<Vec<(Option<i64>, String)>>,
    }

    impl StubClient {
        fn new(batches: Vec<Vec<store::EventRecord>>) -> Self {
            Self {
                batches: RefCell::new(batches),
                queries: RefCell::new(Vec::new()),
            }
        }
    }

    impl ChatClient for StubClient {
        fn daemon_status(&mut self) -> Result<ChatDaemonStatus> {
            // Return a harmless default rather than panic — follower polling
            // could legitimately check daemon status in the future, and tests
            // that don't care about it shouldn't have to override.
            Ok(ChatDaemonStatus::default())
        }

        fn launch_session(&mut self, _request: LaunchRequest) -> Result<store::SessionRecord> {
            unimplemented!("not exercised in follower tests")
        }

        fn get_session(&mut self, _session_id: &str) -> Result<store::SessionRecord> {
            unimplemented!("not exercised in follower tests")
        }

        fn list_sessions(&mut self) -> Result<Vec<store::SessionRecord>> {
            unimplemented!("not exercised in follower tests")
        }

        fn list_events(&mut self, query: ChatEventQuery<'_>) -> Result<Vec<store::EventRecord>> {
            self.queries
                .borrow_mut()
                .push((query.after_seq, query.session_id.to_string()));
            let mut batches = self.batches.borrow_mut();
            if batches.is_empty() {
                Ok(vec![])
            } else {
                Ok(batches.remove(0))
            }
        }

        fn send_input(&mut self, _session_id: &str, _data: &str) -> Result<()> {
            unimplemented!("not exercised in follower tests")
        }

        fn kill_session(&mut self, _session_id: &str) -> Result<()> {
            unimplemented!("not exercised in follower tests")
        }

        fn archive_session(&mut self, _session_id: &str) -> Result<()> {
            unimplemented!("not exercised in follower tests")
        }

        fn summon_session(&mut self, _session_id: &str) -> Result<store::SessionRecord> {
            unimplemented!("not exercised in follower tests")
        }

        fn sacrifice_session(&mut self, _session_id: &str) -> Result<()> {
            unimplemented!("not exercised in follower tests")
        }
    }

    fn output_event(seq: i64, data: &str) -> store::EventRecord {
        store::EventRecord {
            seq,
            id: format!("event-{seq}"),
            session_id: "session-1".to_string(),
            kind: "output".to_string(),
            payload_json: serde_json::json!({ "data": data }).to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        }
    }

    fn exit_event(seq: i64, status: &str, exit_code: Option<i32>) -> store::EventRecord {
        let payload = match exit_code {
            Some(code) => serde_json::json!({ "status": status, "exitCode": code }),
            None => serde_json::json!({ "status": status }),
        };
        store::EventRecord {
            seq,
            id: format!("event-{seq}"),
            session_id: "session-1".to_string(),
            kind: "exit".to_string(),
            payload_json: payload.to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn decode_output_event_extracts_data_chunk() {
        let event = output_event(1, "hello world\n");
        assert_eq!(
            decode_event(&event),
            CastFollowEvent::Output("hello world\n".to_string())
        );
    }

    #[test]
    fn decode_output_event_handles_missing_payload_safely() {
        let mut event = output_event(1, "ignored");
        event.payload_json = "not json".to_string();
        assert_eq!(decode_event(&event), CastFollowEvent::Output(String::new()));
    }

    #[test]
    fn decode_exit_event_extracts_status_and_exit_code() {
        let event = exit_event(7, "completed", Some(0));
        assert_eq!(
            decode_event(&event),
            CastFollowEvent::Exit {
                status: "completed".to_string(),
                exit_code: Some(0),
            }
        );
    }

    #[test]
    fn decode_exit_event_handles_missing_exit_code() {
        let event = exit_event(7, "failed", None);
        assert_eq!(
            decode_event(&event),
            CastFollowEvent::Exit {
                status: "failed".to_string(),
                exit_code: None,
            }
        );
    }

    #[test]
    fn decode_exit_event_accepts_snake_case_exit_code_too() {
        let mut event = exit_event(7, "completed", None);
        event.payload_json =
            serde_json::json!({ "status": "completed", "exit_code": 137 }).to_string();
        assert_eq!(
            decode_event(&event),
            CastFollowEvent::Exit {
                status: "completed".to_string(),
                exit_code: Some(137),
            }
        );
    }

    #[test]
    fn decode_unknown_kind_becomes_other_with_kind_label() {
        let event = store::EventRecord {
            seq: 42,
            id: "event-42".to_string(),
            session_id: "session-1".to_string(),
            kind: "cast.summary".to_string(),
            payload_json: "{}".to_string(),
            created_at: "2026-05-19T00:00:00Z".to_string(),
        };
        assert_eq!(
            decode_event(&event),
            CastFollowEvent::Other {
                kind: "cast.summary".to_string()
            }
        );
    }

    #[test]
    fn cursor_advances_monotonically_and_ignores_rewinds() {
        let mut cursor = CastFollowCursor::new();
        assert_eq!(cursor.after_seq(), None);
        cursor.advance(5);
        assert_eq!(cursor.after_seq(), Some(5));
        cursor.advance(3);
        assert_eq!(cursor.after_seq(), Some(5), "must not rewind");
        cursor.advance(5);
        assert_eq!(cursor.after_seq(), Some(5), "duplicate must not advance");
        cursor.advance(9);
        assert_eq!(cursor.after_seq(), Some(9));
    }

    #[test]
    fn first_poll_sends_no_after_seq_and_returns_decoded_events() {
        let mut client = StubClient::new(vec![vec![
            output_event(1, "alpha\n"),
            output_event(2, "beta\n"),
        ]]);

        let batch = poll_once(&mut client, "session-1", CastFollowCursor::new()).unwrap();

        assert_eq!(
            batch.events,
            vec![
                CastFollowEvent::Output("alpha\n".to_string()),
                CastFollowEvent::Output("beta\n".to_string()),
            ]
        );
        assert_eq!(batch.cursor.after_seq(), Some(2));
        assert!(batch.exit.is_none());

        let queries = client.queries.borrow();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0], (None, "session-1".to_string()));
    }

    #[test]
    fn subsequent_polls_pass_cursor_as_after_seq_to_avoid_duplicates() {
        let mut client = StubClient::new(vec![
            vec![output_event(1, "first\n"), output_event(2, "second\n")],
            vec![output_event(3, "third\n")],
        ]);

        let first =
            poll_once(&mut client, "session-1", CastFollowCursor::new()).expect("first poll");
        let second = poll_once(&mut client, "session-1", first.cursor).expect("second poll");

        assert_eq!(first.cursor.after_seq(), Some(2));
        assert_eq!(second.cursor.after_seq(), Some(3));
        assert_eq!(
            second.events,
            vec![CastFollowEvent::Output("third\n".to_string())]
        );

        let queries = client.queries.borrow();
        assert_eq!(queries[0].0, None);
        assert_eq!(
            queries[1].0,
            Some(2),
            "second poll must request events after seq 2 to dedupe"
        );
    }

    #[test]
    fn exit_event_is_surfaced_in_the_batch_so_caller_can_stop_polling() {
        let mut client = StubClient::new(vec![vec![
            output_event(1, "working\n"),
            exit_event(2, "completed", Some(0)),
        ]]);

        let batch = poll_once(&mut client, "session-1", CastFollowCursor::new()).unwrap();

        assert!(
            batch.exit.is_some(),
            "batch with an exit event must surface it"
        );
        let exit = batch.exit.unwrap();
        assert_eq!(exit.status, "completed");
        assert_eq!(exit.exit_code, Some(0));
        // The exit event still appears in the events list so the renderer
        // can announce it; the caller decides whether to stop polling.
        assert!(matches!(
            batch.events.last(),
            Some(CastFollowEvent::Exit { .. })
        ));
    }

    /// Observer that records every callback so tests can assert on the
    /// sequence of events delivered to the renderer.
    #[derive(Default)]
    struct RecordingObserver {
        output: Vec<String>,
        exits: Vec<(String, Option<i32>)>,
        others: Vec<String>,
    }

    impl FollowerObserver for RecordingObserver {
        fn on_output(&mut self, chunk: &str) {
            self.output.push(chunk.to_string());
        }
        fn on_exit(&mut self, status: &str, exit_code: Option<i32>) {
            self.exits.push((status.to_string(), exit_code));
        }
        fn on_other(&mut self, kind: &str) {
            self.others.push(kind.to_string());
        }
    }

    /// Pacer that allows N idle polls before failing the test. Production
    /// uses a real sleep; this catches infinite-loop bugs in unit tests.
    struct CountingPacer {
        remaining: usize,
    }

    impl FollowerPacer for CountingPacer {
        fn between_polls(&mut self) -> Result<()> {
            if self.remaining == 0 {
                anyhow::bail!("follower idled too many times in tests");
            }
            self.remaining -= 1;
            Ok(())
        }
    }

    #[test]
    fn follow_until_exit_streams_output_and_stops_on_exit_event() {
        let mut client = StubClient::new(vec![
            vec![output_event(1, "starting\n")],
            vec![
                output_event(2, "working\n"),
                exit_event(3, "completed", Some(0)),
            ],
        ]);
        let mut observer = RecordingObserver::default();
        let mut pacer = CountingPacer { remaining: 5 };

        let exit = follow_until_exit(&mut client, "session-1", &mut observer, &mut pacer).unwrap();

        assert_eq!(exit.status, "completed");
        assert_eq!(exit.exit_code, Some(0));
        assert_eq!(observer.output, vec!["starting\n", "working\n"]);
        assert_eq!(observer.exits.len(), 1);
        assert_eq!(observer.exits[0].0, "completed");

        // Two polls in total: one produced the "starting" chunk, the second
        // delivered "working" + exit. The pacer should have been called once
        // (between the two polls) and not after the exit.
        assert_eq!(pacer.remaining, 4);
    }

    #[test]
    fn follow_until_exit_dedupes_via_cursor_across_polls() {
        let mut client = StubClient::new(vec![
            vec![output_event(1, "a\n"), output_event(2, "b\n")],
            vec![output_event(3, "c\n"), exit_event(4, "completed", Some(0))],
        ]);
        let mut observer = RecordingObserver::default();
        let mut pacer = CountingPacer { remaining: 5 };

        follow_until_exit(&mut client, "session-1", &mut observer, &mut pacer).unwrap();

        assert_eq!(observer.output, vec!["a\n", "b\n", "c\n"]);
        let queries = client.queries.borrow();
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].0, None);
        assert_eq!(
            queries[1].0,
            Some(2),
            "the second poll must request events after seq 2 to avoid replaying a/b"
        );
    }

    #[test]
    fn follow_until_exit_records_unknown_event_kinds_then_keeps_going() {
        let mut client = StubClient::new(vec![vec![
            store::EventRecord {
                seq: 1,
                id: "event-1".to_string(),
                session_id: "session-1".to_string(),
                kind: "cast.summary".to_string(),
                payload_json: "{}".to_string(),
                created_at: "2026-05-19T00:00:00Z".to_string(),
            },
            exit_event(2, "completed", Some(0)),
        ]]);
        let mut observer = RecordingObserver::default();
        let mut pacer = CountingPacer { remaining: 1 };

        follow_until_exit(&mut client, "session-1", &mut observer, &mut pacer).unwrap();

        assert_eq!(observer.others, vec!["cast.summary"]);
        assert_eq!(observer.exits.len(), 1);
    }

    #[test]
    fn poll_with_no_events_advances_nothing() {
        let mut client = StubClient::new(vec![vec![]]);
        let mut cursor = CastFollowCursor::new();
        cursor.advance(7);

        let batch = poll_once(&mut client, "session-1", cursor).unwrap();

        assert!(batch.events.is_empty());
        assert_eq!(batch.cursor, cursor, "empty batch must not move cursor");
        assert!(batch.exit.is_none());
    }
}
