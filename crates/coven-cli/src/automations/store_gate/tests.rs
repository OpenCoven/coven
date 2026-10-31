//! The gate's predicate and its commitments, against a dispatched trusted
//! attempt (proposal §4, tests 1 to 6).

use std::sync::{mpsc, Arc, Mutex};
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

/// A store gate whose daemon dies between committing a result and writing
/// it: the commitment stands, and nothing is written until `hold` lets the
/// stand-in thread go.
struct Crashing {
    inner: StoreGate,
    crashed: mpsc::Sender<()>,
    hold: mpsc::Receiver<()>,
}

impl Gate for Crashing {
    fn admit(&mut self) -> std::result::Result<(), Refusal> {
        Gate::admit(&mut self.inner)
    }

    fn commit(&mut self, committing: &Committing<'_>) -> std::result::Result<(), Refusal> {
        Gate::commit(&mut self.inner, committing)?;
        let _ = self.crashed.send(());
        let _ = self.hold.recv();
        Err(Refusal::AttemptClosed)
    }
}

#[test]
fn a_crash_after_commit_keeps_the_row_and_restart_sweeps_the_channel() {
    let dispatched = Dispatched::new();
    let home = dispatched.fixture.home.path().to_path_buf();
    let (crash, crashed) = mpsc::channel();
    let (release, hold) = mpsc::channel::<()>();
    let gate_home = home.clone();
    let prepared = prepare_with_gate(
        dispatched.conn(),
        &dispatched.fixture.launch(&dispatched.launched),
        Utc::now(),
        move |authorized| {
            Ok(Box::new(Crashing {
                inner: StoreGate::open(
                    &gate_home,
                    authorized.held.clone(),
                    Duration::from_millis(200),
                    None,
                )?,
                crashed: crash,
                hold,
            }))
        },
    )
    .expect("the launch is admitted");
    let reading = thread::spawn(move || read_through(prepared, &[read("notes/today.md")]));
    crashed.recv().unwrap();

    // The daemon is gone: the row records that the bytes were authorized,
    // and the channel's directory, with its token, is still on disk.
    let served = |rows: &[(i64, String, Option<String>, Option<String>)]| {
        rows.len() == 1 && rows[0].0 == 1 && rows[0].1 == "served"
    };
    assert!(served(&dispatched.rows()), "{:?}", dispatched.rows());
    let runtime = crate::automations::scoped_read::channel::runtime_dir(&home);
    let channels = || std::fs::read_dir(&runtime).unwrap().count();
    assert_eq!(channels(), 1);

    // The next daemon starts.
    let restarted = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    crate::daemon::recover_orphaned_sessions(&home, &restarted).unwrap();
    crate::daemon::recover_stale_scoped_read_channels(&home);
    assert_eq!(channels(), 0, "the stale channel is swept");
    let (session, attempt): (String, String) = dispatched
        .conn()
        .query_row(
            "SELECT s.status, a.state FROM automation_attempts AS a
             JOIN sessions AS s ON s.id = ?2
             WHERE a.id = ?1",
            rusqlite::params![dispatched.held.attempt_id, dispatched.launched.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (session.as_str(), attempt.as_str()),
        ("orphaned", "started"),
        "the attempt is held for recovery"
    );
    assert!(served(&dispatched.rows()));

    // No byte of the committed result was ever written.
    drop(release);
    let results = reading.join().unwrap();
    assert_eq!(results, [(json!({ "refusal": "attempt_closed" }), true)]);

    // Settlement after the restart (#1608): the channel never closed, so
    // the committed row reconciles with nothing, and the run neither
    // settles nor gets a receipt.
    assert_eq!(
        crate::automations::scoped_read_settlement::reconcile(
            dispatched.conn(),
            &dispatched.held.attempt_id,
            None
        )
        .unwrap(),
        Err(crate::automations::scoped_read_settlement::Discrepancy::NotClosed)
    );
    let report =
        crate::automations::runner::settle_finished_runs(dispatched.conn(), Utc::now()).unwrap();
    assert_eq!(report.succeeded, 0, "{report:?}");
    let (status, receipt): (String, Option<String>) = dispatched
        .conn()
        .query_row(
            "SELECT status, receipt_id FROM automation_runs WHERE id = ?1",
            [&dispatched.run],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_ne!(status, "succeeded");
    assert!(receipt.is_none(), "no receipt for a crashed attempt");
}

/// The run's deadline crossing a write lock wait (#1605). A test clock P is
/// evaluated against, and a trace of what happened in which order.
#[derive(Clone)]
struct Timeline {
    now: Arc<Mutex<DateTime<Utc>>>,
    trace: Arc<Mutex<Vec<String>>>,
}

impl Timeline {
    /// A timeline whose run, `dispatched`'s, ends at a fixed `timeout_at` an
    /// hour away, and whose clock reads a millisecond before it.
    fn new(dispatched: &Dispatched) -> (Self, DateTime<Utc>) {
        let timeout_at = DateTime::parse_from_rfc3339(
            &(Utc::now() + TimeDelta::hours(1)).to_rfc3339_opts(SecondsFormat::Millis, true),
        )
        .unwrap()
        .with_timezone(&Utc);
        dispatched
            .conn()
            .execute(
                "UPDATE automation_runs SET timeout_at = ?2 WHERE id = ?1",
                rusqlite::params![
                    dispatched.run,
                    timeout_at.to_rfc3339_opts(SecondsFormat::Millis, true)
                ],
            )
            .unwrap();
        let timeline = Self {
            now: Arc::new(Mutex::new(timeout_at - TimeDelta::milliseconds(1))),
            trace: Arc::default(),
        };
        (timeline, timeout_at)
    }

    fn set(&self, now: DateTime<Utc>) {
        *self.now.lock().unwrap() = now;
    }

    fn note(&self, event: String) {
        self.trace.lock().unwrap().push(event);
    }

    fn trace(&self) -> Vec<String> {
        self.trace.lock().unwrap().clone()
    }

    /// `gate`, reading this clock and noting each time it does, and calling
    /// `locking` before it asks for the write lock.
    fn drive(&self, mut gate: StoreGate, locking: impl FnMut() + Send + 'static) -> StoreGate {
        let timeline = self.clone();
        gate.clock.replaced = Some(Box::new(move || {
            let now = *timeline.now.lock().unwrap();
            timeline.note(format!(
                "gate read the time: {}",
                now.to_rfc3339_opts(SecondsFormat::Millis, true)
            ));
            now
        }));
        gate.locking = Some(Box::new(locking));
        gate
    }
}

/// Holds the store's write lock from another connection until `release`.
fn hold_write_lock(dispatched: &Dispatched) -> Connection {
    let other =
        crate::store::open_store(&crate::api::store_path(dispatched.fixture.home.path())).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    other
}

fn committed_at(dispatched: &Dispatched) -> Vec<String> {
    let mut statement = dispatched
        .conn()
        .prepare("SELECT committed_at FROM automation_scoped_read_commitments ORDER BY sequence")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The gate asks for the write lock while the run still has time; another
/// writer holds the lock; the run's `timeout_at` passes; the writer lets go;
/// the gate takes the lock. P must be evaluated at the serialization point,
/// so the result is withheld, even though the monotonic deadline is far off.
#[test]
fn a_deadline_that_passes_during_the_write_lock_wait_withholds_the_result() {
    let dispatched = Dispatched::new();
    let (timeline, timeout_at) = Timeline::new(&dispatched);
    let deadline = Some(Instant::now() + Duration::from_secs(60));
    let gate = StoreGate::open(
        dispatched.fixture.home.path(),
        dispatched.held.clone(),
        Duration::from_secs(10),
        deadline,
    )
    .unwrap();
    let (asked, asking) = mpsc::channel();
    let noted = timeline.clone();
    let mut gate = timeline.drive(gate, move || {
        noted.note("gate asked for the write lock".to_owned());
        let _ = asked.send(());
    });
    assert_eq!(gate.admit(), Ok(()));
    timeline.trace.lock().unwrap().clear();

    let holder = hold_write_lock(&dispatched);
    timeline.note("another writer holds the write lock".to_owned());
    let committing = thread::spawn(move || gate.commit(&served(1)));
    asking.recv().unwrap();
    timeline.set(timeout_at);
    timeline.note(format!(
        "the run's timeout_at passed: {}",
        timeout_at.to_rfc3339_opts(SecondsFormat::Millis, true)
    ));
    holder.execute_batch("COMMIT").unwrap();
    timeline.note("the other writer let go".to_owned());
    let verdict = committing.join().unwrap();

    let trace = timeline.trace();
    assert_eq!(
        verdict,
        Err(Refusal::AttemptClosed),
        "served after the deadline: {trace:#?}"
    );
    assert_eq!(dispatched.rows(), [withheld(1)], "{trace:#?}");
    let expired = timeout_at.to_rfc3339_opts(SecondsFormat::Millis, true);
    assert_eq!(committed_at(&dispatched), std::slice::from_ref(&expired));
    assert_eq!(
        trace,
        [
            "another writer holds the write lock".to_owned(),
            "gate asked for the write lock".to_owned(),
            format!("the run's timeout_at passed: {expired}"),
            "the other writer let go".to_owned(),
            format!("gate read the time: {expired}"),
        ],
        "the gate reads the time only once it holds the lock"
    );
}

/// The same ordering over the attempt's channel: no byte of the result
/// reaches the harness and the service stops.
#[test]
fn over_the_channel_a_deadline_that_passes_during_the_wait_writes_no_byte() {
    let dispatched = Dispatched::new();
    let (timeline, timeout_at) = Timeline::new(&dispatched);
    let (asked, asking) = mpsc::channel();
    let home = dispatched.fixture.home.path().to_path_buf();
    let driving = timeline.clone();
    let prepared = prepare_with_gate(
        dispatched.conn(),
        &dispatched.fixture.launch(&dispatched.launched),
        Utc::now(),
        move |authorized| {
            let gate = StoreGate::open(
                &home,
                authorized.held.clone(),
                Duration::from_secs(10),
                None,
            )?;
            Ok(Box::new(driving.drive(gate, move || {
                let _ = asked.send(());
            })))
        },
    )
    .expect("the launch is admitted");
    let holder = hold_write_lock(&dispatched);
    let reading = thread::spawn(move || {
        read_through(prepared, &[read("notes/today.md"), read("notes/today.md")])
    });
    asking.recv().unwrap();
    timeline.set(timeout_at);
    holder.execute_batch("COMMIT").unwrap();
    let results = reading.join().unwrap();
    assert_eq!(
        results,
        [(json!({ "refusal": "attempt_closed" }), true)],
        "the result is withheld and the second call is never served"
    );
    assert!(!format!("{results:?}").contains(GRANTED));
    assert_eq!(dispatched.rows(), [withheld(1)]);
}

/// At exactly `timeout_at` the run's time is up; a millisecond before, it is
/// not. A commitment made in time stands, with its own time, and the next
/// call after the deadline is refused.
#[test]
fn a_commitment_before_the_deadline_stands_and_one_at_it_is_withheld() {
    let dispatched = Dispatched::new();
    let (timeline, timeout_at) = Timeline::new(&dispatched);
    let mut gate = timeline.drive(dispatched.gate(), || {});
    assert_eq!(gate.admit(), Ok(()));
    assert_eq!(gate.commit(&served(1)), Ok(()));
    timeline.set(timeout_at);
    assert_eq!(gate.admit(), Err(Refusal::AttemptClosed));
    assert_eq!(gate.commit(&served(2)), Err(Refusal::AttemptClosed));
    assert_eq!(
        dispatched.rows(),
        [
            (1, "served".to_owned(), None, Some(DIGEST.to_owned())),
            withheld(2),
        ]
    );
    assert_eq!(
        committed_at(&dispatched),
        [
            (timeout_at - TimeDelta::milliseconds(1)).to_rfc3339_opts(SecondsFormat::Millis, true),
            timeout_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        ]
    );
}

/// A revocation the lock holder commits while the gate waits wins, with the
/// run's time still left.
#[test]
fn a_revocation_committed_during_the_write_lock_wait_withholds_the_result() {
    let dispatched = Dispatched::new();
    let (timeline, _timeout_at) = Timeline::new(&dispatched);
    let (asked, asking) = mpsc::channel();
    let mut gate = timeline.drive(dispatched.gate_for(dispatched.held.clone()), move || {
        let _ = asked.send(());
    });
    gate.busy = Duration::from_secs(10);
    let holder = hold_write_lock(&dispatched);
    let committing = thread::spawn(move || gate.commit(&served(1)));
    asking.recv().unwrap();
    holder
        .execute(
            "UPDATE familiar_ledger_revisions
             SET status = 'revoked', revoked_at = '2026-10-27T00:00:00.000Z',
                 revocation_reason = 'compromised'
             WHERE revision_id = ?1",
            [dispatched.launched.projection.identity_revision_id.as_str()],
        )
        .unwrap();
    holder.execute_batch("COMMIT").unwrap();
    assert_eq!(committing.join().unwrap(), Err(Refusal::AttemptClosed));
    assert_eq!(dispatched.rows(), [withheld(1)]);
}

/// A wait never outlasts the run's monotonic deadline: once it has passed
/// the gate refuses at once, records nothing, and serves nothing, however
/// long the store stays busy.
#[test]
fn a_wait_past_the_run_deadline_refuses_unrecorded_without_waiting() {
    let dispatched = Dispatched::new();
    let (timeline, timeout_at) = Timeline::new(&dispatched);
    let gate = StoreGate::open(
        dispatched.fixture.home.path(),
        dispatched.held.clone(),
        Duration::from_secs(30),
        Some(Instant::now()),
    )
    .unwrap();
    let mut gate = timeline.drive(gate, || {});
    let holder = hold_write_lock(&dispatched);
    timeline.set(timeout_at);
    let started = Instant::now();
    assert_eq!(gate.commit(&served(1)), Err(Refusal::AttemptClosed));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the wait is bounded by the deadline, not the 30s busy limit"
    );
    holder.execute_batch("ROLLBACK").unwrap();
    assert_eq!(dispatched.rows(), []);
}
