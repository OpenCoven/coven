//! Reconciling a scoped read attempt's commitments with its channel's close
//! (#1606).

use std::io::{self, Cursor, Write};
use std::time::Duration;

use chrono::Utc;
use rusqlite::Connection;
use serde_json::json;

use super::*;
use crate::automations::scoped_read::service::{Committing, Gate, Limits, Service};
use crate::automations::scoped_read::Refusal;
use crate::automations::store_gate::StoreGate;
use crate::automations::trusted_launch::test_support::{
    r0, read, read_through, scope, Fixture, GRANTED,
};
use crate::automations::trusted_launch::{launch_scopes, prepare, prepare_with_gate};

const ATTEMPT: &str = "attempt-1";
const CHANNEL: &str = "channel-1";
const DIGEST: &str = "abababababababababababababababababababababababababababababababab";
const OTHER: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

/// A store holding only what reconciliation reads.
fn store() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE automation_attempts (id TEXT PRIMARY KEY);")
        .unwrap();
    conn.execute_batch(crate::automations::store_gate::SCHEMA)
        .unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    conn.execute(
        "INSERT INTO automation_attempts (id) VALUES (?1)",
        [ATTEMPT],
    )
    .unwrap();
    conn
}

/// A commitment row, as the gate writes it.
fn commit(
    conn: &Connection,
    channel: &str,
    sequence: i64,
    outcome: &str,
    refusal: Option<&str>,
    digest: Option<&str>,
) {
    conn.execute(
        "INSERT INTO automation_scoped_read_commitments (
            attempt_id, channel_id, sequence, tool, outcome, refusal,
            result_digest, result_bytes, committed_at
         ) VALUES (?1, ?2, ?3, 'read_file', ?4, ?5, ?6, ?7, '2026-10-30T00:00:00.000Z')",
        rusqlite::params![
            ATTEMPT,
            channel,
            sequence,
            outcome,
            refusal,
            digest,
            digest.map(|_| 42)
        ],
    )
    .unwrap();
}

fn served(sequence: u64, digest: &str) -> Operation {
    Operation {
        sequence,
        tool: Some("read_file"),
        outcome: Outcome::Served {
            digest: digest.to_owned(),
        },
        written: true,
    }
}

fn refused(sequence: u64, refusal: Refusal) -> Operation {
    Operation {
        sequence,
        tool: Some("read_file"),
        outcome: Outcome::Refused {
            refusal,
            digest: DIGEST.to_owned(),
        },
        written: true,
    }
}

fn withheld(sequence: u64) -> Operation {
    Operation {
        sequence,
        tool: Some("read_file"),
        outcome: Outcome::Withheld {
            refusal: Refusal::AttemptClosed,
        },
        written: true,
    }
}

fn close(conn: &Connection, channel: &str, operations: &[Operation]) {
    record(conn, ATTEMPT, channel, operations, Utc::now()).unwrap();
}

fn reconciled(conn: &Connection) -> std::result::Result<usize, Discrepancy> {
    reconcile(conn, ATTEMPT, None).unwrap()
}

#[test]
fn matching_commitments_and_close_reconcile() {
    let conn = store();
    commit(&conn, CHANNEL, 1, "served", None, Some(DIGEST));
    commit(
        &conn,
        CHANNEL,
        2,
        "refused",
        Some("scope_missing"),
        Some(DIGEST),
    );
    close(
        &conn,
        CHANNEL,
        &[served(1, DIGEST), refused(2, Refusal::ScopeMissing)],
    );
    assert_eq!(reconciled(&conn), Ok(2));
    // The stream must show every operation the service ran.
    assert_eq!(reconcile(&conn, ATTEMPT, Some(2)).unwrap(), Ok(2));
    assert_eq!(reconcile(&conn, ATTEMPT, Some(3)).unwrap(), Ok(2));
    assert_eq!(
        reconcile(&conn, ATTEMPT, Some(1)).unwrap(),
        Err(Discrepancy::OperationNotInStream)
    );
}

/// A withheld operation's records can agree, committed or not: a gate that
/// refused at admit, or found the store busy, records nothing. They still
/// never settle a run, because the authority it would rest on ended or could
/// not be confirmed (#1608). A disagreement elsewhere is still reported as
/// itself.
#[test]
fn a_withheld_operation_agrees_but_never_reconciles() {
    let committed = store();
    commit(&committed, CHANNEL, 1, "served", None, Some(DIGEST));
    commit(
        &committed,
        CHANNEL,
        2,
        "withheld",
        Some("attempt_closed"),
        None,
    );
    close(&committed, CHANNEL, &[served(1, DIGEST), withheld(2)]);
    assert_eq!(reconciled(&committed), Err(Discrepancy::OperationWithheld));
    assert_eq!(
        reconcile(&committed, ATTEMPT, Some(1)).unwrap(),
        Err(Discrepancy::OperationNotInStream)
    );

    let unrecorded = store();
    commit(&unrecorded, CHANNEL, 1, "served", None, Some(DIGEST));
    close(&unrecorded, CHANNEL, &[served(1, DIGEST), withheld(2)]);
    assert_eq!(reconciled(&unrecorded), Err(Discrepancy::OperationWithheld));

    let alone = store();
    close(&alone, CHANNEL, &[withheld(1)]);
    assert_eq!(reconciled(&alone), Err(Discrepancy::OperationWithheld));
}

#[test]
fn an_unused_channel_reconciles_with_no_operations() {
    let conn = store();
    close(&conn, CHANNEL, &[]);
    assert_eq!(reconciled(&conn), Ok(0));
}

/// Each way the records can disagree, and what reconciliation says.
#[test]
fn missing_reordered_duplicated_tampered_or_mismatched_evidence_does_not_reconcile() {
    type Setup = fn(&Connection);
    let cases: Vec<(&str, Setup, Discrepancy)> = vec![
        (
            "a channel that never closed",
            |conn| commit(conn, CHANNEL, 1, "served", None, Some(DIGEST)),
            Discrepancy::NotClosed,
        ),
        (
            "an attempt with no record at all",
            |_| {},
            Discrepancy::NotClosed,
        ),
        (
            "two closed channels",
            |conn| {
                close(conn, CHANNEL, &[]);
                close(conn, "channel-2", &[]);
            },
            Discrepancy::SeveralChannels,
        ),
        (
            "a commitment from a channel that never closed",
            |conn| {
                commit(conn, CHANNEL, 1, "served", None, Some(DIGEST));
                commit(conn, "channel-crashed", 1, "served", None, Some(DIGEST));
                close(conn, CHANNEL, &[served(1, DIGEST)]);
            },
            Discrepancy::ForeignChannel,
        ),
        (
            "a commitment the close does not list",
            |conn| {
                commit(conn, CHANNEL, 1, "served", None, Some(DIGEST));
                commit(conn, CHANNEL, 2, "served", None, Some(DIGEST));
                close(conn, CHANNEL, &[served(1, DIGEST)]);
            },
            Discrepancy::CommitmentUnaccounted,
        ),
        (
            "a served operation with no commitment",
            |conn| close(conn, CHANNEL, &[served(1, DIGEST)]),
            Discrepancy::OperationUncommitted,
        ),
        (
            "other bytes committed",
            |conn| {
                commit(conn, CHANNEL, 1, "served", None, Some(OTHER));
                close(conn, CHANNEL, &[served(1, DIGEST)]);
            },
            Discrepancy::DigestMismatch,
        ),
        (
            "a refusal committed as served",
            |conn| {
                commit(conn, CHANNEL, 1, "served", None, Some(DIGEST));
                close(conn, CHANNEL, &[refused(1, Refusal::ScopeMissing)]);
            },
            Discrepancy::OutcomeMismatch,
        ),
        (
            "another refusal committed",
            |conn| {
                commit(
                    conn,
                    CHANNEL,
                    1,
                    "refused",
                    Some("scope_excluded"),
                    Some(DIGEST),
                );
                close(conn, CHANNEL, &[refused(1, Refusal::ScopeMissing)]);
            },
            Discrepancy::RefusalMismatch,
        ),
        (
            "a result committed but withheld by the service",
            |conn| {
                commit(conn, CHANNEL, 1, "served", None, Some(DIGEST));
                close(conn, CHANNEL, &[withheld(1)]);
            },
            Discrepancy::OutcomeMismatch,
        ),
        (
            "another tool committed",
            |conn| {
                commit(conn, CHANNEL, 1, "served", None, Some(DIGEST));
                let mut operation = served(1, DIGEST);
                operation.tool = Some("search");
                close(conn, CHANNEL, &[operation]);
            },
            Discrepancy::ToolMismatch,
        ),
        (
            "a committed result that was never written",
            |conn| {
                commit(conn, CHANNEL, 1, "served", None, Some(DIGEST));
                let mut operation = served(1, DIGEST);
                operation.written = false;
                close(conn, CHANNEL, &[operation]);
            },
            Discrepancy::CommittedNotWritten,
        ),
        (
            "a gap in the sequence",
            |conn| {
                commit(conn, CHANNEL, 2, "served", None, Some(DIGEST));
                close(conn, CHANNEL, &[served(2, DIGEST)]);
            },
            Discrepancy::SequenceInvalid,
        ),
        (
            "a duplicated sequence",
            |conn| {
                commit(conn, CHANNEL, 1, "served", None, Some(DIGEST));
                close(conn, CHANNEL, &[served(1, DIGEST), served(1, DIGEST)]);
            },
            Discrepancy::SequenceInvalid,
        ),
        (
            "a reordered sequence",
            |conn| {
                commit(conn, CHANNEL, 1, "served", None, Some(DIGEST));
                commit(conn, CHANNEL, 2, "served", None, Some(DIGEST));
                close(conn, CHANNEL, &[served(2, DIGEST), served(1, DIGEST)]);
            },
            Discrepancy::SequenceInvalid,
        ),
        (
            "an operation after a withheld one",
            |conn| {
                commit(conn, CHANNEL, 2, "served", None, Some(DIGEST));
                close(conn, CHANNEL, &[withheld(1), served(2, DIGEST)]);
            },
            Discrepancy::SequenceInvalid,
        ),
        (
            "a close whose count disagrees with its list",
            |conn| {
                conn.execute(
                    "INSERT INTO automation_scoped_read_closes VALUES (?1, ?2, 1, '[]', 'now')",
                    [ATTEMPT, CHANNEL],
                )
                .unwrap();
            },
            Discrepancy::CloseInvalid,
        ),
        (
            "a close that is not its record",
            |conn| {
                conn.execute(
                    "INSERT INTO automation_scoped_read_closes VALUES (?1, ?2, 0, '{}', 'now')",
                    [ATTEMPT, CHANNEL],
                )
                .unwrap();
            },
            Discrepancy::CloseInvalid,
        ),
        (
            "a close with a field it never writes",
            |conn| {
                let text = json!([{
                    "sequence": 1, "tool": "read_file", "outcome": "served",
                    "refusal": null, "digest": DIGEST, "written": true, "delivered": true
                }])
                .to_string();
                conn.execute(
                    "INSERT INTO automation_scoped_read_closes VALUES (?1, ?2, 1, ?3, 'now')",
                    [ATTEMPT, CHANNEL, &text],
                )
                .unwrap();
                commit(conn, CHANNEL, 1, "served", None, Some(DIGEST));
            },
            Discrepancy::CloseInvalid,
        ),
        (
            "a served operation without a digest",
            |conn| {
                let text = json!([{
                    "sequence": 1, "tool": "read_file", "outcome": "served",
                    "refusal": null, "digest": null, "written": true
                }])
                .to_string();
                conn.execute(
                    "INSERT INTO automation_scoped_read_closes VALUES (?1, ?2, 1, ?3, 'now')",
                    [ATTEMPT, CHANNEL, &text],
                )
                .unwrap();
            },
            Discrepancy::CloseInvalid,
        ),
    ];
    for (what, setup, expected) in cases {
        let conn = store();
        setup(&conn);
        assert_eq!(reconciled(&conn), Err(expected), "{what}");
    }
}

#[test]
fn a_store_without_the_tables_has_not_closed() {
    let conn = Connection::open_in_memory().unwrap();
    assert_eq!(reconciled(&conn), Err(Discrepancy::NotClosed));
}

#[test]
fn closes_cannot_be_updated_or_deleted() {
    let conn = store();
    close(&conn, CHANNEL, &[]);
    for sql in [
        "UPDATE automation_scoped_read_closes SET operations_json = '[]'",
        "DELETE FROM automation_scoped_read_closes",
    ] {
        let error = conn.execute(sql, []).unwrap_err().to_string();
        assert!(error.contains("scoped read closes are"), "{sql}: {error}");
    }
}

/// A dispatched `notes` attempt and the store gate its launch would run
/// under, with a short store wait.
fn dispatched() -> (Fixture, StoreGate) {
    let fixture = Fixture::new();
    fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
    let (_run, launched) = fixture.dispatch("notes");
    let held = launch_scopes(&fixture.home.conn, &fixture.launch(&launched), Utc::now())
        .expect("the dispatched attempt is authorized")
        .held;
    let gate = StoreGate::open(fixture.home.path(), held, Duration::from_millis(100), None)
        .expect("the gate opens");
    (fixture, gate)
}

fn attempt_of(fixture: &Fixture) -> String {
    fixture
        .home
        .conn
        .query_row("SELECT id FROM automation_attempts", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn a_launch_through_its_channel_records_a_close_that_reconciles() {
    let fixture = Fixture::new();
    fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
    let (_run, launched) = fixture.dispatch("notes");
    let prepared = prepare(&fixture.home.conn, &fixture.launch(&launched), Utc::now())
        .expect("the launch is admitted");
    let results = read_through(prepared, &[read("notes/today.md"), read("note.txt")]);
    assert_eq!(results[0].0["content"], GRANTED);
    assert_eq!(results[1].0["refusal"], "scope_missing");
    let attempt = attempt_of(&fixture);
    assert_eq!(
        reconcile(&fixture.home.conn, &attempt, Some(2)).unwrap(),
        Ok(2)
    );
    assert_eq!(
        reconcile(&fixture.home.conn, &attempt, Some(1)).unwrap(),
        Err(Discrepancy::OperationNotInStream),
        "an operation the harness's stream does not show"
    );
}

#[test]
fn a_launch_no_relay_reached_records_an_empty_close() {
    let fixture = Fixture::new();
    fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
    let (_run, launched) = fixture.dispatch("notes");
    let prepared = prepare(&fixture.home.conn, &fixture.launch(&launched), Utc::now())
        .expect("the launch is admitted");
    prepared.finish();
    assert_eq!(
        reconcile(&fixture.home.conn, &attempt_of(&fixture), None).unwrap(),
        Ok(0)
    );
}

/// The store gate, but the daemon dies after the first commitment: nothing
/// is written and the channel never closes.
struct Crashing(StoreGate);

impl Gate for Crashing {
    fn admit(&mut self) -> std::result::Result<(), Refusal> {
        Gate::admit(&mut self.0)
    }

    fn commit(&mut self, committing: &Committing<'_>) -> std::result::Result<(), Refusal> {
        Gate::commit(&mut self.0, committing)?;
        Err(Refusal::AttemptClosed)
    }

    // A dead daemon records no close.
}

#[test]
fn a_crash_after_commit_leaves_the_attempt_unreconciled_and_restart_invents_nothing() {
    let fixture = Fixture::new();
    fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
    let (_run, launched) = fixture.dispatch("notes");
    let home = fixture.home.path().to_path_buf();
    let prepared = prepare_with_gate(
        &fixture.home.conn,
        &fixture.launch(&launched),
        Utc::now(),
        move |authorized| {
            Ok(Box::new(Crashing(StoreGate::open(
                &home,
                authorized.held.clone(),
                Duration::from_millis(200),
                None,
            )?)))
        },
    )
    .expect("the launch is admitted");
    let results = read_through(prepared, &[read("notes/today.md")]);
    assert_eq!(results, [(json!({ "refusal": "attempt_closed" }), true)]);
    let attempt = attempt_of(&fixture);
    let conn = &fixture.home.conn;
    // The bytes were authorized, never delivered, and nothing says otherwise.
    assert_eq!(
        reconcile(conn, &attempt, None).unwrap(),
        Err(Discrepancy::NotClosed)
    );

    // The next daemon starts and recovers the session: it opens no channel,
    // adopts none and writes no close, so the attempt stays unreconciled.
    let restarted = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    crate::daemon::recover_orphaned_sessions(fixture.home.path(), &restarted).unwrap();
    crate::daemon::recover_stale_scoped_read_channels(fixture.home.path());
    assert_eq!(
        reconcile(conn, &attempt, None).unwrap(),
        Err(Discrepancy::NotClosed)
    );
    let closes: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM automation_scoped_read_closes",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(closes, 0);
}

/// Accepts the initialize reply and then fails every write, as a relay that
/// went away would.
struct FailingAfter {
    left: usize,
    written: Vec<u8>,
}

impl Write for FailingAfter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        self.written.extend_from_slice(bytes);
        if bytes.contains(&b'\n') {
            self.left -= 1;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_committed_result_that_fails_to_write_is_recorded_unwritten_and_does_not_reconcile() {
    let (fixture, gate) = dispatched();
    let serve = Box::new(|_: &serde_json::Value| {
        Err::<crate::automations::scoped_read::open::Response, _>(Refusal::ScopeMissing)
    });
    let service = Service::with_serve(
        serve,
        Box::new(gate),
        Limits {
            operation: Duration::from_secs(5),
            deadline: None,
        },
    );
    let mut input = Vec::new();
    for message in [
        json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18" } }),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "read_file", "arguments": read("note.txt") } }),
    ] {
        input.extend(serde_json::to_vec(&message).unwrap());
        input.push(b'\n');
    }
    let mut output = FailingAfter {
        left: 1,
        written: Vec::new(),
    };
    let operations = service.run(Cursor::new(input), &mut output);
    assert_eq!(operations.len(), 1);
    assert!(!operations[0].written, "the reply never reached the relay");
    assert!(!String::from_utf8_lossy(&output.written).contains("scope_missing"));
    let attempt = attempt_of(&fixture);
    let conn = &fixture.home.conn;
    let outcome: String = conn
        .query_row(
            "SELECT outcome FROM automation_scoped_read_commitments",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(outcome, "refused", "the result was authorized");
    assert_eq!(
        reconcile(conn, &attempt, None).unwrap(),
        Err(Discrepancy::CommittedNotWritten)
    );
}

#[test]
fn a_close_the_busy_store_cannot_take_is_not_recorded() {
    let (fixture, mut gate) = dispatched();
    let other = crate::store::open_store(&crate::api::store_path(fixture.home.path())).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    Gate::close(&mut gate, &[]);
    other.execute_batch("ROLLBACK").unwrap();
    let attempt = attempt_of(&fixture);
    assert_eq!(
        reconcile(&fixture.home.conn, &attempt, None).unwrap(),
        Err(Discrepancy::NotClosed)
    );
    let log = std::fs::read_to_string(fixture.home.path().join("daemon-recovery.log"))
        .unwrap_or_default();
    assert!(log.contains("closed unrecorded"), "{log}");
}

#[test]
fn closes_hold_no_content_path_pattern_or_name() {
    const CONTENT: &str = "SENTINEL-CONTENT-4be0";
    const NAME: &str = "SENTINEL-PATH-d21c";
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.project.join("notes/deep")).unwrap();
    std::fs::write(
        fixture.project.join(format!("notes/deep/{NAME}.md")),
        format!("{CONTENT}\n"),
    )
    .unwrap();
    fixture.create("notes", r0(vec![scope("notes/deep", true)]), "ACTIVE");
    let (_run, launched) = fixture.dispatch("notes");
    let prepared = prepare(&fixture.home.conn, &fixture.launch(&launched), Utc::now())
        .expect("the launch is admitted");
    let results = read_through(
        prepared,
        &[
            read(&format!("notes/deep/{NAME}.md")),
            json!({ "tool": "list_directory", "root": "project", "path": "notes/deep" }),
            json!({ "tool": "search", "root": "project", "path": "notes/deep",
                    "pattern": CONTENT }),
        ],
    );
    assert!(results[0].0.to_string().contains(CONTENT), "{results:?}");
    let row: (String, String, String) = fixture
        .home
        .conn
        .query_row(
            "SELECT attempt_id, channel_id, operations_json FROM automation_scoped_read_closes",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    for column in [&row.0, &row.1, &row.2] {
        assert!(!column.contains("SENTINEL"), "a close discloses {column}");
    }
    assert_eq!(
        reconcile(&fixture.home.conn, &attempt_of(&fixture), Some(3)).unwrap(),
        Ok(3)
    );
}
