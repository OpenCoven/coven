//! The gate's predicate and its commitments, against a dispatched trusted
//! attempt (proposal §4, tests 1 to 6).

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, TimeDelta, Utc};
use serde_json::json;

use super::*;
use crate::automations::scoped_read::service::{Committing, Gate};
use crate::automations::trusted_launch::test_support::{
    r0, read, read_through, scope, Fixture, Launched, GRANTED,
};
use crate::automations::trusted_launch::{launch_scopes, prepare, prepare_with_gate};

const DIGEST: &str = "abababababababababababababababababababababababababababababababab";

/// A dispatched `notes` attempt and what its launch verified.
struct Dispatched {
    fixture: Fixture,
    run: String,
    launched: Launched,
    held: Held,
}

impl Dispatched {
    fn new() -> Self {
        let fixture = Fixture::new();
        fixture.create("notes", r0(vec![scope("notes/today.md", false)]), "ACTIVE");
        let (run, launched) = fixture.dispatch("notes");
        let held = launch_scopes(&fixture.home.conn, &fixture.launch(&launched), Utc::now())
            .expect("the dispatched attempt is authorized")
            .held;
        Self {
            fixture,
            run,
            launched,
            held,
        }
    }

    fn gate(&self) -> StoreGate {
        self.gate_for(self.held.clone())
    }

    fn gate_for(&self, held: Held) -> StoreGate {
        StoreGate::open(
            self.fixture.home.path(),
            held,
            Duration::from_millis(200),
            None,
        )
        .expect("the gate opens")
    }

    fn conn(&self) -> &Connection {
        &self.fixture.home.conn
    }

    /// Every commitment row: sequence, outcome, refusal and digest.
    fn rows(&self) -> Vec<(i64, String, Option<String>, Option<String>)> {
        let mut statement = self
            .conn()
            .prepare(
                "SELECT sequence, outcome, refusal, result_digest
                 FROM automation_scoped_read_commitments ORDER BY committed_at, sequence",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
}

fn served(sequence: u64) -> Committing<'static> {
    Committing {
        sequence,
        tool: Some("read_file"),
        refusal: None,
        digest: DIGEST,
        bytes: 42,
    }
}

fn withheld(sequence: i64) -> (i64, String, Option<String>, Option<String>) {
    (
        sequence,
        "withheld".to_owned(),
        Some("attempt_closed".to_owned()),
        None,
    )
}

/// A way to withdraw a running attempt's authority, and its name.
type Withdraw = (&'static str, fn(&Dispatched));

/// Each source that can withdraw a running attempt's authority.
fn withdrawals() -> Vec<Withdraw> {
    fn revoke(dispatched: &Dispatched, index: usize) {
        let key = &dispatched.held.key_ids[index];
        assert!(crate::automations::authority_keys::revoke_key(
            dispatched.conn(),
            dispatched.fixture.home.path(),
            key,
            "test",
            Utc::now(),
        )
        .unwrap());
    }
    vec![
        ("dispatch-authority key revoked", |d| revoke(d, 0)),
        ("threads-decision key revoked", |d| revoke(d, 1)),
        ("owner-principal key revoked", |d| revoke(d, 2)),
        ("familiar revision revoked", |d| {
            d.conn()
                .execute(
                    "UPDATE familiar_ledger_revisions
                     SET status = 'revoked', revoked_at = '2026-10-27T00:00:00.000Z',
                         revocation_reason = 'compromised'
                     WHERE revision_id = ?1",
                    [d.launched.projection.identity_revision_id.as_str()],
                )
                .unwrap();
        }),
        ("familiar revision retired", |d| {
            d.conn()
                .execute(
                    "UPDATE familiar_ledger_revisions SET status = 'retired'
                     WHERE revision_id = ?1",
                    [d.launched.projection.identity_revision_id.as_str()],
                )
                .unwrap();
        }),
        ("definition paused", |d| {
            d.conn()
                .execute(
                    "UPDATE automation_definitions SET lifecycle_state = 'paused'
                     WHERE id = 'notes'",
                    [],
                )
                .unwrap();
        }),
        ("stop fence claimed", |d| {
            crate::automations::runner::claim_stop_fence(
                d.conn(),
                &d.run,
                &d.launched.id,
                "timeout",
                None,
                None,
                Utc::now(),
            )
            .unwrap();
        }),
        ("run timed out", |d| {
            let past =
                (Utc::now() - TimeDelta::seconds(1)).to_rfc3339_opts(SecondsFormat::Millis, true);
            d.conn()
                .execute(
                    "UPDATE automation_runs SET timeout_at = ?2 WHERE id = ?1",
                    rusqlite::params![d.run, past],
                )
                .unwrap();
        }),
    ]
}

#[test]
fn a_revocation_that_commits_first_withholds_the_result() {
    for (what, withdraw) in withdrawals() {
        let dispatched = Dispatched::new();
        let mut gate = dispatched.gate();
        assert_eq!(gate.admit(), Ok(()), "{what}");
        withdraw(&dispatched);
        assert_eq!(
            gate.commit(&served(1)),
            Err(Refusal::AttemptClosed),
            "{what}"
        );
        assert_eq!(dispatched.rows(), [withheld(1)], "{what}");
        assert_eq!(gate.admit(), Err(Refusal::AttemptClosed), "{what}");
    }
}

#[test]
fn a_commitment_that_commits_first_stands_and_the_next_call_is_refused() {
    let dispatched = Dispatched::new();
    let mut gate = dispatched.gate();
    assert_eq!(gate.admit(), Ok(()));
    assert_eq!(gate.commit(&served(1)), Ok(()));
    let refused = Committing {
        refusal: Some(Refusal::ScopeMissing),
        ..served(2)
    };
    assert_eq!(gate.commit(&refused), Ok(()));
    withdrawals()[1].1(&dispatched);
    assert_eq!(gate.admit(), Err(Refusal::AttemptClosed));
    assert_eq!(gate.commit(&served(3)), Err(Refusal::AttemptClosed));
    assert_eq!(
        dispatched.rows(),
        [
            (1, "served".to_owned(), None, Some(DIGEST.to_owned())),
            (
                2,
                "refused".to_owned(),
                Some("scope_missing".to_owned()),
                Some(DIGEST.to_owned())
            ),
            withheld(3),
        ]
    );
}

#[test]
fn a_wrong_attempt_session_or_binding_is_refused_at_admit_and_commit() {
    let dispatched = Dispatched::new();
    let mut other_session = dispatched.held.clone();
    other_session.session_id = "session-elsewhere".to_owned();
    let mut other_attempt = dispatched.held.clone();
    other_attempt.attempt_id = "attempt-elsewhere".to_owned();
    for (what, held) in [("session", other_session), ("attempt", other_attempt)] {
        let mut gate = dispatched.gate_for(held);
        assert_eq!(gate.admit(), Err(Refusal::AttemptClosed), "{what}");
        assert_eq!(
            gate.commit(&served(1)),
            Err(Refusal::AttemptClosed),
            "{what}"
        );
    }
    assert!(
        !dispatched.rows().iter().any(|row| row.1 == "served"),
        "nothing is served under another attempt's authority"
    );

    // The attempt's pinned binding is rewritten after the launch verified it.
    let mut gate = dispatched.gate();
    let conn = dispatched.conn();
    conn.execute_batch("DROP TRIGGER automation_attempt_authority_immutable;")
        .unwrap();
    conn.execute(
        "UPDATE automation_attempts
         SET authority_extension_json = authority_extension_json || ' '
         WHERE id = ?1",
        [&dispatched.held.attempt_id],
    )
    .unwrap();
    assert_eq!(gate.admit(), Err(Refusal::AttemptClosed));
    assert_eq!(gate.commit(&served(1)), Err(Refusal::AttemptClosed));
    assert!(!dispatched.rows().iter().any(|row| row.1 == "served"));
}

#[test]
fn a_busy_store_withholds_the_result_unrecorded() {
    let dispatched = Dispatched::new();
    let mut gate = dispatched.gate();
    assert_eq!(gate.admit(), Ok(()));
    let other =
        crate::store::open_store(&crate::api::store_path(dispatched.fixture.home.path())).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = Instant::now();
    assert_eq!(gate.commit(&served(1)), Err(Refusal::AttemptClosed));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the wait is bounded"
    );
    other.execute_batch("ROLLBACK").unwrap();
    assert_eq!(dispatched.rows(), []);
}

#[test]
fn commitments_cannot_be_updated_or_deleted() {
    let dispatched = Dispatched::new();
    let mut gate = dispatched.gate();
    assert_eq!(gate.commit(&served(1)), Ok(()));
    let conn = dispatched.conn();
    let update = conn
        .execute(
            "UPDATE automation_scoped_read_commitments SET outcome = 'refused'",
            [],
        )
        .unwrap_err();
    assert!(update.to_string().contains("immutable"), "{update}");
    let delete = conn
        .execute("DELETE FROM automation_scoped_read_commitments", [])
        .unwrap_err();
    assert!(delete.to_string().contains("retained"), "{delete}");
    assert_eq!(dispatched.rows().len(), 1);
}

/// A store gate that stops before each step until the test resumes it, so
/// the test can commit a revocation at exactly that point.
struct Pausing {
    inner: StoreGate,
    at: mpsc::Sender<&'static str>,
    resume: mpsc::Receiver<()>,
}

impl Gate for Pausing {
    fn admit(&mut self) -> std::result::Result<(), Refusal> {
        let _ = self.at.send("admit");
        let _ = self.resume.recv();
        Gate::admit(&mut self.inner)
    }

    fn commit(&mut self, committing: &Committing<'_>) -> std::result::Result<(), Refusal> {
        let _ = self.at.send("commit");
        let _ = self.resume.recv();
        Gate::commit(&mut self.inner, committing)
    }
}

/// The reading thread, where the gate paused, and the switch that resumes
/// it.
struct Paused {
    reading: thread::JoinHandle<Vec<(serde_json::Value, bool)>>,
    paused: mpsc::Receiver<&'static str>,
    resume: mpsc::Sender<()>,
}

/// Launches `dispatched` under a pausing store gate and reads the granted
/// note twice through its channel, on another thread.
fn paused_reads(dispatched: &Dispatched) -> Paused {
    let (at, paused) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let home = dispatched.fixture.home.path().to_path_buf();
    let prepared = prepare_with_gate(
        dispatched.conn(),
        &dispatched.fixture.launch(&dispatched.launched),
        Utc::now(),
        move |authorized| {
            Ok(Box::new(Pausing {
                inner: StoreGate::open(
                    &home,
                    authorized.held.clone(),
                    Duration::from_millis(200),
                    None,
                )?,
                at,
                resume: resumed,
            }))
        },
    )
    .expect("the launch is admitted");
    let reading = thread::spawn(move || {
        read_through(prepared, &[read("notes/today.md"), read("notes/today.md")])
    });
    Paused {
        reading,
        paused,
        resume,
    }
}

#[test]
fn over_the_channel_a_revocation_first_writes_no_byte_and_stops_the_service() {
    for (what, withdraw) in withdrawals() {
        let dispatched = Dispatched::new();
        let Paused {
            reading,
            paused,
            resume,
        } = paused_reads(&dispatched);
        assert_eq!(paused.recv().unwrap(), "admit", "{what}");
        resume.send(()).unwrap();
        assert_eq!(paused.recv().unwrap(), "commit", "{what}");
        withdraw(&dispatched);
        resume.send(()).unwrap();
        // Any later pause runs straight on.
        drop((paused, resume));
        let results = reading.join().unwrap();
        assert_eq!(
            results,
            [(json!({ "refusal": "attempt_closed" }), true)],
            "{what}: the result is withheld and the second call is never served"
        );
        assert_eq!(dispatched.rows(), [withheld(1)], "{what}");
    }
}

#[test]
fn over_the_channel_a_commitment_first_is_delivered_and_the_next_call_refused() {
    let dispatched = Dispatched::new();
    let Paused {
        reading,
        paused,
        resume,
    } = paused_reads(&dispatched);
    for step in ["admit", "commit"] {
        assert_eq!(paused.recv().unwrap(), step);
        resume.send(()).unwrap();
    }
    // The first result committed; the second call has not been admitted.
    assert_eq!(paused.recv().unwrap(), "admit");
    withdrawals()[1].1(&dispatched);
    resume.send(()).unwrap();
    drop((paused, resume));
    let results = reading.join().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0["content"], GRANTED);
    assert!(!results[0].1);
    assert_eq!(results[1], (json!({ "refusal": "attempt_closed" }), true));
    let rows = dispatched.rows();
    assert_eq!(rows.len(), 1, "admit records nothing: {rows:?}");
    assert_eq!((rows[0].0, rows[0].1.as_str()), (1, "served"));
    assert!(rows[0].3.is_some());
}

#[test]
fn commitments_hold_no_content_path_pattern_or_name() {
    const CONTENT: &str = "SENTINEL-CONTENT-91c2";
    const NAME: &str = "SENTINEL-PATH-7f3a";
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
            read(&format!("notes/deep/{NAME}-missing.md")),
        ],
    );
    assert!(results[0].0.to_string().contains(CONTENT), "{results:?}");
    assert!(results[1].0.to_string().contains(NAME), "{results:?}");
    assert!(results[2].0.to_string().contains(CONTENT), "{results:?}");
    assert_eq!(results[3].0["refusal"], "resource_not_found");

    let conn = &fixture.home.conn;
    let mut statement = conn
        .prepare("SELECT * FROM automation_scoped_read_commitments")
        .unwrap();
    let columns = statement.column_count();
    let rows: Vec<Vec<String>> = statement
        .query_map([], |row| {
            (0..columns)
                .map(|index| {
                    Ok(match row.get_ref(index)? {
                        rusqlite::types::ValueRef::Text(text) => {
                            String::from_utf8_lossy(text).into_owned()
                        }
                        other => format!("{other:?}"),
                    })
                })
                .collect()
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 4, "every call is committed: {rows:?}");
    for row in &rows {
        for column in row {
            assert!(
                !column.contains("SENTINEL"),
                "a commitment discloses {column}"
            );
        }
    }
}
